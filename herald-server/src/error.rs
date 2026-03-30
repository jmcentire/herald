use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum HeraldError {
    #[error("redis error: {0}")]
    Redis(#[from] redis::RedisError),

    #[error("encryption error: {0}")]
    Encryption(String),

    #[error("rate limit exceeded")]
    RateLimited,

    #[error("queue full")]
    QueueFull,

    #[error("payload too large: {size} bytes exceeds {limit} byte limit")]
    PayloadTooLarge { size: usize, limit: usize },

    #[error("not found: {0}")]
    NotFound(String),

    #[error("unauthorized: {0}")]
    Unauthorized(String),

    #[error("bad request: {0}")]
    BadRequest(String),

    #[error("internal error: {0}")]
    Internal(String),
}

impl IntoResponse for HeraldError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            HeraldError::Redis(e) => {
                tracing::error!(error = %e, "redis error");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal error")
            }
            HeraldError::Encryption(e) => {
                tracing::error!(error = %e, "encryption error");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal error")
            }
            HeraldError::RateLimited => (StatusCode::TOO_MANY_REQUESTS, "rate limit exceeded"),
            HeraldError::QueueFull => {
                (StatusCode::INSUFFICIENT_STORAGE, "queue full")
            }
            HeraldError::PayloadTooLarge { .. } => {
                (StatusCode::PAYLOAD_TOO_LARGE, "payload too large")
            }
            HeraldError::NotFound(_) => (StatusCode::NOT_FOUND, "not found"),
            HeraldError::Unauthorized(_) => (StatusCode::UNAUTHORIZED, "unauthorized"),
            HeraldError::BadRequest(_) => (StatusCode::BAD_REQUEST, "bad request"),
            HeraldError::Internal(e) => {
                tracing::error!(error = %e, "internal error");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal error")
            }
        };

        let body = json!({ "error": message });
        (status, axum::Json(body)).into_response()
    }
}
