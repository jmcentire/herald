#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error("config error: {0}")]
    Config(String),

    #[error("http error: {0}")]
    Http(String),

    #[error("websocket error: {0}")]
    WebSocket(String),

    #[error("handler error: {0}")]
    Handler(String),

    #[error("hook error: {0}")]
    Hook(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}
