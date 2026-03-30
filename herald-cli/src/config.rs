use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
pub struct Config {
    pub server: String,
    pub api_key: String,

    #[serde(default = "default_connection")]
    pub connection: ConnectionMode,

    #[serde(default = "default_poll_interval")]
    pub poll_interval: String,

    #[serde(default = "default_max_concurrent")]
    pub max_concurrent: usize,

    #[serde(default)]
    pub handlers: HashMap<String, Handler>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ConnectionMode {
    Poll,
    Websocket,
}

fn default_connection() -> ConnectionMode {
    ConnectionMode::Poll
}

fn default_poll_interval() -> String {
    "10s".to_string()
}

fn default_max_concurrent() -> usize {
    3
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Handler {
    pub command: String,

    #[serde(default)]
    pub args: Vec<String>,

    /// Template for stdin. Supports {{.body}}, {{.headers}}, {{.message_id}},
    /// {{.endpoint}}, {{.received_at}}.
    pub prompt_template: Option<String>,

    /// What to pipe to stdin: "body", "prompt" (rendered template), or "none"
    #[serde(default = "default_stdin")]
    pub stdin: StdinMode,

    #[serde(default = "default_timeout")]
    pub timeout: String,

    #[serde(default = "default_on_failure")]
    pub on_failure: FailureAction,

    #[serde(default)]
    pub env: HashMap<String, String>,

    #[serde(default)]
    pub hooks: Hooks,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum StdinMode {
    Body,
    Prompt,
    None,
}

fn default_stdin() -> StdinMode {
    StdinMode::Body
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum FailureAction {
    Nack,
    NackPermanent,
}

fn default_on_failure() -> FailureAction {
    FailureAction::Nack
}

fn default_timeout() -> String {
    "300s".to_string()
}

#[derive(Debug, Default, Deserialize, Serialize, Clone)]
pub struct Hooks {
    pub pre: Option<HookConfig>,
    pub post: Option<HookConfig>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct HookConfig {
    pub command: String,

    #[serde(default)]
    pub args: Vec<String>,

    /// What to pipe to the hook's stdin: "body", "payload" (alias), "summary" (handler stdout)
    #[serde(default = "default_hook_stdin")]
    pub stdin: HookStdinMode,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum HookStdinMode {
    Body,
    Payload,
    Summary,
    None,
}

fn default_hook_stdin() -> HookStdinMode {
    HookStdinMode::Body
}

/// Parse a duration string like "10s", "5m", "1h"
pub fn parse_duration(s: &str) -> std::time::Duration {
    let s = s.trim();
    if let Some(secs) = s.strip_suffix('s') {
        std::time::Duration::from_secs(secs.parse().unwrap_or(10))
    } else if let Some(mins) = s.strip_suffix('m') {
        std::time::Duration::from_secs(mins.parse::<u64>().unwrap_or(1) * 60)
    } else if let Some(hours) = s.strip_suffix('h') {
        std::time::Duration::from_secs(hours.parse::<u64>().unwrap_or(1) * 3600)
    } else {
        std::time::Duration::from_secs(s.parse().unwrap_or(10))
    }
}

impl Config {
    pub fn load(path: &PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
        let content = std::fs::read_to_string(path)?;
        let config: Config = serde_yaml::from_str(&content)?;
        Ok(config)
    }

    pub fn default_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("herald")
            .join("config.yaml")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_duration_seconds() {
        assert_eq!(parse_duration("30s"), std::time::Duration::from_secs(30));
    }

    #[test]
    fn test_parse_duration_minutes() {
        assert_eq!(parse_duration("5m"), std::time::Duration::from_secs(300));
    }

    #[test]
    fn test_parse_duration_hours() {
        assert_eq!(parse_duration("1h"), std::time::Duration::from_secs(3600));
    }

    #[test]
    fn test_parse_duration_bare_number() {
        assert_eq!(parse_duration("60"), std::time::Duration::from_secs(60));
    }

    #[test]
    fn test_parse_config() {
        let yaml = r#"
server: https://proxy.herald.tools
api_key: hrl_sk_test
connection: websocket
max_concurrent: 5
handlers:
  github:
    command: claude
    args: ["-p"]
    prompt_template: "Handle: {{.body}}"
    stdin: prompt
    timeout: 60s
    on_failure: nack_permanent
    hooks:
      pre:
        command: kindex
        args: ["ingest", "--stdin"]
        stdin: body
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.server, "https://proxy.herald.tools");
        assert_eq!(config.connection, ConnectionMode::Websocket);
        assert_eq!(config.max_concurrent, 5);

        let handler = config.handlers.get("github").unwrap();
        assert_eq!(handler.command, "claude");
        assert_eq!(handler.stdin, StdinMode::Prompt);
        assert_eq!(handler.on_failure, FailureAction::NackPermanent);
        assert!(handler.hooks.pre.is_some());
    }
}
