use crate::config::Config;

/// Shared application state, available to all route handlers.
#[derive(Clone)]
pub struct AppState {
    pub redis: redis::aio::MultiplexedConnection,
    pub config: Config,
}
