use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::client::QueueMessage;
use crate::config::{Config, FailureAction};
use crate::error::CliError;
use crate::handler;

/// WebSocket message types matching the server protocol.
#[derive(serde::Serialize)]
#[serde(tag = "type")]
#[serde(rename_all = "snake_case")]
enum ClientMsg {
    Auth { api_key: String },
    Ack { message_id: String },
    Nack { message_id: String, permanent: bool },
}

#[derive(serde::Deserialize, Debug)]
#[serde(tag = "type")]
#[serde(rename_all = "snake_case")]
enum ServerMsg {
    AuthOk,
    AuthError { reason: String },
    Message {
        message_id: String,
        body: String,
        headers: Option<serde_json::Value>,
        received_at: String,
        deliver_count: u32,
    },
    AckOk { message_id: String },
    Error { reason: String },
}

/// Run WebSocket streaming for all configured endpoints.
/// Spawns one WebSocket connection per endpoint.
pub async fn run(config: &Config) -> Result<(), CliError> {
    let mut handles = Vec::new();

    for (endpoint, handler_config) in &config.handlers {
        let ws_url = build_ws_url(&config.server, endpoint);
        let api_key = config.api_key.clone();
        let handler_cfg = handler_config.clone();
        let endpoint = endpoint.clone();

        let handle = tokio::spawn(async move {
            loop {
                tracing::info!(endpoint = %endpoint, "connecting WebSocket");

                match run_single_stream(&ws_url, &api_key, &endpoint, &handler_cfg).await {
                    Ok(()) => {
                        tracing::info!(endpoint = %endpoint, "WebSocket closed cleanly");
                    }
                    Err(e) => {
                        tracing::error!(endpoint = %endpoint, error = %e, "WebSocket error");
                    }
                }

                // Reconnect with backoff
                tracing::info!(endpoint = %endpoint, "reconnecting in 5s");
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        });

        handles.push(handle);
    }

    // Wait for all streams (they run forever with reconnect)
    for handle in handles {
        let _ = handle.await;
    }

    Ok(())
}

async fn run_single_stream(
    ws_url: &str,
    api_key: &str,
    endpoint: &str,
    handler_config: &crate::config::Handler,
) -> Result<(), CliError> {
    let (ws_stream, _) = tokio_tungstenite::connect_async(ws_url)
        .await
        .map_err(|e| CliError::WebSocket(format!("connect failed: {e}")))?;

    let (mut write, mut read) = ws_stream.split();

    // First-message authentication
    let auth_msg = serde_json::to_string(&ClientMsg::Auth {
        api_key: api_key.to_string(),
    })
    .map_err(|e| CliError::WebSocket(e.to_string()))?;

    write
        .send(WsMessage::Text(auth_msg.into()))
        .await
        .map_err(|e| CliError::WebSocket(format!("auth send failed: {e}")))?;

    // Wait for auth response
    let auth_response = read
        .next()
        .await
        .ok_or_else(|| CliError::WebSocket("connection closed during auth".into()))?
        .map_err(|e| CliError::WebSocket(format!("auth read failed: {e}")))?;

    if let WsMessage::Text(text) = auth_response {
        let msg: ServerMsg = serde_json::from_str(&text)
            .map_err(|e| CliError::WebSocket(format!("auth parse failed: {e}")))?;

        match msg {
            ServerMsg::AuthOk => {
                tracing::info!(endpoint = %endpoint, "WebSocket authenticated");
            }
            ServerMsg::AuthError { reason } => {
                return Err(CliError::WebSocket(format!("auth rejected: {reason}")));
            }
            _ => {
                return Err(CliError::WebSocket("unexpected auth response".into()));
            }
        }
    } else {
        return Err(CliError::WebSocket("expected text auth response".into()));
    }

    // Message loop
    while let Some(ws_msg) = read.next().await {
        let ws_msg = ws_msg.map_err(|e| CliError::WebSocket(e.to_string()))?;

        let text = match ws_msg {
            WsMessage::Text(t) => t,
            WsMessage::Close(_) => break,
            _ => continue,
        };

        let server_msg: ServerMsg = match serde_json::from_str(&text) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(error = %e, "failed to parse server message");
                continue;
            }
        };

        match server_msg {
            ServerMsg::Message {
                message_id,
                body,
                headers,
                received_at,
                deliver_count,
            } => {
                let queue_msg = QueueMessage {
                    message_id: message_id.clone(),
                    fingerprint: String::new(),
                    body,
                    headers,
                    received_at,
                    deliver_count,
                    encryption: "service".into(),
                    key_version: None,
                };

                let result = handler::run_handler(handler_config, &queue_msg).await;

                let response = match result {
                    Ok(hr) if hr.success => ClientMsg::Ack { message_id },
                    Ok(hr) => ClientMsg::Nack {
                        message_id,
                        permanent: hr.permanent_failure,
                    },
                    Err(e) => {
                        tracing::error!(error = %e, "handler error");
                        ClientMsg::Nack {
                            message_id,
                            permanent: handler_config.on_failure == FailureAction::NackPermanent,
                        }
                    }
                };

                let response_text = serde_json::to_string(&response)
                    .map_err(|e| CliError::WebSocket(e.to_string()))?;

                write
                    .send(WsMessage::Text(response_text.into()))
                    .await
                    .map_err(|e| CliError::WebSocket(format!("send failed: {e}")))?;
            }
            ServerMsg::AckOk { message_id } => {
                tracing::debug!(message_id = %message_id, "ACK confirmed");
            }
            ServerMsg::Error { reason } => {
                tracing::warn!(reason = %reason, "server error");
            }
            _ => {}
        }
    }

    Ok(())
}

fn build_ws_url(server: &str, endpoint: &str) -> String {
    let ws_scheme = if server.starts_with("https") {
        "wss"
    } else {
        "ws"
    };
    let host = server
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    format!("{ws_scheme}://{host}/stream/{endpoint}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_ws_url_https() {
        assert_eq!(
            build_ws_url("https://proxy.herald.tools", "github"),
            "wss://proxy.herald.tools/stream/github"
        );
    }

    #[test]
    fn test_build_ws_url_http() {
        assert_eq!(
            build_ws_url("http://localhost:8080", "test"),
            "ws://localhost:8080/stream/test"
        );
    }
}
