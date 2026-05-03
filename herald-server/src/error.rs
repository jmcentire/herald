use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
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

/// Default Retry-After (seconds) for queue-full and rate-limit responses.
const RETRY_AFTER_SECS: u64 = 60;

impl IntoResponse for HeraldError {
    fn into_response(self) -> Response {
        let (status, type_, code, message, retry_after) = match &self {
            HeraldError::Redis(e) => {
                tracing::error!(error = %e, "redis error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "internal_error",
                    "internal error".to_string(),
                    None,
                )
            }
            HeraldError::Encryption(e) => {
                tracing::error!(error = %e, "encryption error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "internal_error",
                    "internal error".to_string(),
                    None,
                )
            }
            HeraldError::RateLimited => (
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limit_exceeded",
                "rate_limit_exceeded",
                "rate limit exceeded".to_string(),
                Some(RETRY_AFTER_SECS),
            ),
            HeraldError::QueueFull => (
                StatusCode::TOO_MANY_REQUESTS,
                "resource_exhausted",
                "queue_depth_exceeded",
                "queue depth limit reached; ACK existing messages to make room"
                    .to_string(),
                Some(RETRY_AFTER_SECS),
            ),
            HeraldError::PayloadTooLarge { size, limit } => (
                StatusCode::PAYLOAD_TOO_LARGE,
                "invalid_request",
                "payload_too_large",
                format!("payload of {size} bytes exceeds {limit} byte limit"),
                None,
            ),
            HeraldError::NotFound(msg) => (
                StatusCode::NOT_FOUND,
                "invalid_request",
                "not_found",
                msg.clone(),
                None,
            ),
            HeraldError::Unauthorized(msg) => (
                StatusCode::UNAUTHORIZED,
                "authentication_error",
                "unauthorized",
                msg.clone(),
                None,
            ),
            HeraldError::BadRequest(msg) => (
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "bad_request",
                msg.clone(),
                None,
            ),
            HeraldError::Internal(e) => {
                tracing::error!(error = %e, "internal error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "internal_error",
                    "internal error".to_string(),
                    None,
                )
            }
        };

        let body = json!({
            "error": {
                "type": type_,
                "code": code,
                "message": message,
            }
        });

        let mut headers = HeaderMap::new();
        if let Some(secs) = retry_after {
            if let Ok(v) = HeaderValue::from_str(&secs.to_string()) {
                headers.insert(header::RETRY_AFTER, v);
            }
        }

        (status, headers, axum::Json(body)).into_response()
    }
}
