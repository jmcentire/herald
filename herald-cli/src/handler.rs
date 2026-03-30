use std::process::Stdio;
use std::time::Duration;

use base64::Engine as _;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::timeout;

use crate::client::QueueMessage;
use crate::config::{
    parse_duration, FailureAction, Handler, HookConfig, HookStdinMode, StdinMode,
};
use crate::error::CliError;

/// Result of running a handler on a message.
pub struct HandlerResult {
    pub success: bool,
    pub stdout: String,
    pub permanent_failure: bool,
}

/// Execute a handler for a received message.
pub async fn run_handler(handler: &Handler, msg: &QueueMessage) -> Result<HandlerResult, CliError> {
    let timeout_dur = parse_duration(&handler.timeout);

    // Decode body from base64
    let body_bytes = base64::engine::general_purpose::STANDARD
        .decode(&msg.body)
        .unwrap_or_else(|_| msg.body.as_bytes().to_vec());

    let body_str = String::from_utf8_lossy(&body_bytes);

    // Run pre hook
    if let Some(ref pre_hook) = handler.hooks.pre {
        tracing::info!(hook = "pre", command = %pre_hook.command, "running pre hook");
        if let Err(e) = run_hook(pre_hook, msg, &body_str, "").await {
            tracing::error!(error = %e, "pre hook failed");
            return Err(CliError::Hook(format!("pre hook failed: {e}")));
        }
    }

    // Prepare stdin content
    let stdin_content = match handler.stdin {
        StdinMode::Body => body_str.to_string(),
        StdinMode::Prompt => render_template(
            handler.prompt_template.as_deref().unwrap_or("{{.body}}"),
            msg,
            &body_str,
        ),
        StdinMode::None => String::new(),
    };

    // Build command
    let mut cmd = Command::new(&handler.command);
    cmd.args(&handler.args);

    // Set environment variables with template rendering
    for (key, val) in &handler.env {
        let rendered = render_template(val, msg, &body_str);
        cmd.env(key, rendered);
    }

    // Stdin handling
    if handler.stdin != StdinMode::None {
        cmd.stdin(Stdio::piped());
    } else {
        cmd.stdin(Stdio::null());
    }
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    tracing::info!(
        command = %handler.command,
        message_id = %msg.message_id,
        "spawning handler"
    );

    let mut child = cmd.spawn().map_err(|e| {
        CliError::Handler(format!("failed to spawn '{}': {e}", handler.command))
    })?;

    // Write stdin
    if handler.stdin != StdinMode::None {
        if let Some(mut stdin) = child.stdin.take() {
            let content = stdin_content.clone();
            tokio::spawn(async move {
                let _ = stdin.write_all(content.as_bytes()).await;
                let _ = stdin.shutdown().await;
            });
        }
    }

    // Wait for exit with timeout
    let result = timeout(timeout_dur, child.wait_with_output()).await;

    match result {
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr);

            if !stderr.is_empty() {
                tracing::debug!(
                    message_id = %msg.message_id,
                    stderr = %stderr,
                    "handler stderr"
                );
            }

            let success = output.status.success();
            let permanent_failure =
                !success && handler.on_failure == FailureAction::NackPermanent;

            tracing::info!(
                message_id = %msg.message_id,
                exit_code = output.status.code().unwrap_or(-1),
                success,
                "handler completed"
            );

            // Run post hook (failure doesn't affect ACK)
            if let Some(ref post_hook) = handler.hooks.post {
                tracing::info!(hook = "post", command = %post_hook.command, "running post hook");
                if let Err(e) = run_hook(post_hook, msg, &body_str, &stdout).await {
                    tracing::warn!(error = %e, "post hook failed (non-fatal)");
                }
            }

            Ok(HandlerResult {
                success,
                stdout,
                permanent_failure,
            })
        }
        Ok(Err(e)) => Err(CliError::Handler(format!("process error: {e}"))),
        Err(_) => {
            // Timeout — kill the process
            tracing::warn!(
                message_id = %msg.message_id,
                timeout = ?timeout_dur,
                "handler timed out, killing"
            );
            // child is dropped here which sends SIGKILL
            Ok(HandlerResult {
                success: false,
                stdout: String::new(),
                permanent_failure: handler.on_failure == FailureAction::NackPermanent,
            })
        }
    }
}

/// Run a pre or post hook.
async fn run_hook(
    hook: &HookConfig,
    _msg: &QueueMessage,
    body: &str,
    handler_stdout: &str,
) -> Result<(), CliError> {
    let stdin_content = match hook.stdin {
        HookStdinMode::Body | HookStdinMode::Payload => body.to_string(),
        HookStdinMode::Summary => handler_stdout.to_string(),
        HookStdinMode::None => String::new(),
    };

    let mut cmd = Command::new(&hook.command);
    cmd.args(&hook.args);

    if hook.stdin != HookStdinMode::None {
        cmd.stdin(Stdio::piped());
    } else {
        cmd.stdin(Stdio::null());
    }
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd.spawn().map_err(|e| {
        CliError::Hook(format!("failed to spawn '{}': {e}", hook.command))
    })?;

    if hook.stdin != HookStdinMode::None {
        if let Some(mut stdin) = child.stdin.take() {
            let content = stdin_content;
            tokio::spawn(async move {
                let _ = stdin.write_all(content.as_bytes()).await;
                let _ = stdin.shutdown().await;
            });
        }
    }

    let hook_timeout = Duration::from_secs(30);
    let result = timeout(hook_timeout, child.wait_with_output()).await;

    match result {
        Ok(Ok(output)) if output.status.success() => Ok(()),
        Ok(Ok(output)) => Err(CliError::Hook(format!(
            "hook '{}' exited with code {}",
            hook.command,
            output.status.code().unwrap_or(-1)
        ))),
        Ok(Err(e)) => Err(CliError::Hook(format!("hook process error: {e}"))),
        Err(_) => Err(CliError::Hook(format!(
            "hook '{}' timed out after {hook_timeout:?}",
            hook.command
        ))),
    }
}

/// Render a template string with message variables.
/// Supports: {{.body}}, {{.headers}}, {{.message_id}}, {{.endpoint}}, {{.received_at}}
fn render_template(template: &str, msg: &QueueMessage, body: &str) -> String {
    let headers_str = msg
        .headers
        .as_ref()
        .map(|h| serde_json::to_string(h).unwrap_or_default())
        .unwrap_or_default();

    template
        .replace("{{.body}}", body)
        .replace("{{.headers}}", &headers_str)
        .replace("{{.message_id}}", &msg.message_id)
        .replace("{{.endpoint}}", &msg.fingerprint) // endpoint isn't in QueueMessage, use fingerprint context
        .replace("{{.received_at}}", &msg.received_at)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_msg() -> QueueMessage {
        QueueMessage {
            message_id: "abc123".into(),
            fingerprint: "fp456".into(),
            body: base64::engine::general_purpose::STANDARD.encode(b"hello world"),
            headers: Some(serde_json::json!({"content-type": "application/json"})),
            received_at: "1234567890".into(),
            deliver_count: 1,
            encryption: "service".into(),
            key_version: None,
        }
    }

    #[test]
    fn test_render_template_body() {
        let msg = test_msg();
        let result = render_template("Payload: {{.body}}", &msg, "hello world");
        assert_eq!(result, "Payload: hello world");
    }

    #[test]
    fn test_render_template_message_id() {
        let msg = test_msg();
        let result = render_template("ID: {{.message_id}}", &msg, "");
        assert_eq!(result, "ID: abc123");
    }

    #[test]
    fn test_render_template_headers() {
        let msg = test_msg();
        let result = render_template("H: {{.headers}}", &msg, "");
        assert!(result.contains("content-type"));
    }

    #[test]
    fn test_render_template_multiple() {
        let msg = test_msg();
        let result = render_template(
            "Use kindex for {{.message_id}}. Body: {{.body}}",
            &msg,
            "the payload",
        );
        assert_eq!(result, "Use kindex for abc123. Body: the payload");
    }
}
