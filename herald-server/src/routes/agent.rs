use axum::extract::{Path, Query, State};
use axum::response::IntoResponse;
use axum::Json;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::auth;
use crate::crypto;
use crate::error::HeraldError;
use crate::queue;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct PollParams {
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(default = "default_visibility_timeout")]
    pub visibility_timeout: u64,
}

fn default_limit() -> usize {
    10
}

fn default_visibility_timeout() -> u64 {
    300
}

#[derive(Debug, Serialize)]
pub struct MessageResponse {
    pub message_id: String,
    pub fingerprint: String,
    pub body: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<serde_json::Value>,
    pub received_at: String,
    pub deliver_count: u32,
    pub encryption: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_version: Option<String>,
}

/// GET /queue/:endpoint_name — poll for messages
pub async fn poll_messages(
    State(state): State<AppState>,
    Path(endpoint_name): Path<String>,
    Query(params): Query<PollParams>,
    req: axum::extract::Request,
) -> Result<impl IntoResponse, HeraldError> {
    let api_key = auth::extract_api_key(&req)?;
    let mut conn = state.redis.clone();
    let account = auth::lookup_account(&mut conn, &api_key).await?;
    let limits = account.tier.limits();

    let limit = params.limit.min(100);
    let visibility_timeout = params.visibility_timeout.clamp(30, 43200);

    let messages = queue::fetch(
        &mut conn,
        &account.customer_id,
        &endpoint_name,
        limit,
        visibility_timeout,
    )
    .await?;

    if messages.is_empty() {
        return Ok(axum::http::StatusCode::NO_CONTENT.into_response());
    }

    let mut responses = Vec::with_capacity(messages.len());
    for msg in messages {
        // Decrypt body based on per-message encryption label
        let plaintext_body = if msg.encryption == "none" {
            msg.body.clone()
        } else {
            crypto::decrypt(&state.config.service_encryption_key, &msg.body)?
        };
        let body_b64 =
            base64::engine::general_purpose::STANDARD.encode(&plaintext_body);

        // Decrypt headers if tier allows
        let headers = if limits.headers_included {
            msg.headers
                .as_ref()
                .and_then(|h| {
                    base64::engine::general_purpose::STANDARD.decode(h).ok()
                })
                .and_then(|encrypted| {
                    crypto::decrypt(&state.config.service_encryption_key, &encrypted).ok()
                })
                .and_then(|decrypted| String::from_utf8(decrypted).ok())
                .and_then(|json_str| serde_json::from_str(&json_str).ok())
        } else {
            None
        };

        responses.push(MessageResponse {
            message_id: msg.message_id,
            fingerprint: msg.fingerprint,
            body: body_b64,
            headers,
            received_at: msg.received_at.to_string(),
            deliver_count: msg.deliver_count,
            encryption: msg.encryption,
            key_version: msg.key_version,
        });
    }

    Ok(Json(json!({ "messages": responses })).into_response())
}

/// POST /ack/:endpoint_name/:message_id — acknowledge a single message
pub async fn ack_message(
    State(state): State<AppState>,
    Path((endpoint_name, message_id)): Path<(String, String)>,
    req: axum::extract::Request,
) -> Result<impl IntoResponse, HeraldError> {
    let api_key = auth::extract_api_key(&req)?;
    let mut conn = state.redis.clone();
    let account = auth::lookup_account(&mut conn, &api_key).await?;

    let acked = queue::ack(&mut conn, &account.customer_id, &endpoint_name, &message_id).await?;

    if !acked {
        return Err(HeraldError::NotFound(format!(
            "message {message_id} not in flight"
        )));
    }

    Ok(Json(json!({ "acknowledged": true })))
}

#[derive(Debug, Deserialize)]
pub struct BatchAckRequest {
    pub message_ids: Vec<String>,
}

/// POST /ack/:endpoint_name — batch acknowledge messages
pub async fn batch_ack_messages(
    State(state): State<AppState>,
    Path(endpoint_name): Path<String>,
    req: axum::extract::Request,
) -> Result<impl IntoResponse, HeraldError> {
    // Extract auth first, then body
    let api_key = auth::extract_api_key(&req)?;
    let body = axum::body::to_bytes(req.into_body(), 1024 * 1024)
        .await
        .map_err(|e| HeraldError::BadRequest(e.to_string()))?;
    let batch: BatchAckRequest =
        serde_json::from_slice(&body).map_err(|e| HeraldError::BadRequest(e.to_string()))?;

    let mut conn = state.redis.clone();
    let account = auth::lookup_account(&mut conn, &api_key).await?;

    let mut acknowledged = Vec::new();
    let mut failed = Vec::new();

    for msg_id in &batch.message_ids {
        match queue::ack(&mut conn, &account.customer_id, &endpoint_name, msg_id).await {
            Ok(true) => acknowledged.push(msg_id.clone()),
            _ => failed.push(msg_id.clone()),
        }
    }

    Ok(Json(json!({
        "acknowledged": acknowledged,
        "failed": failed,
    })))
}

#[derive(Debug, Deserialize)]
pub struct NackParams {
    #[serde(default)]
    pub permanent: bool,
}

/// POST /nack/:endpoint_name/:message_id — negative acknowledge
pub async fn nack_message(
    State(state): State<AppState>,
    Path((endpoint_name, message_id)): Path<(String, String)>,
    Query(params): Query<NackParams>,
    req: axum::extract::Request,
) -> Result<impl IntoResponse, HeraldError> {
    let api_key = auth::extract_api_key(&req)?;
    let mut conn = state.redis.clone();
    let account = auth::lookup_account(&mut conn, &api_key).await?;

    let max_retries = 3; // Default, configurable per endpoint later
    let nacked = queue::nack(
        &mut conn,
        &account.customer_id,
        &endpoint_name,
        &message_id,
        params.permanent,
        max_retries,
    )
    .await?;

    if !nacked {
        return Err(HeraldError::NotFound(format!(
            "message {message_id} not in flight"
        )));
    }

    if params.permanent {
        Ok(Json(json!({ "dlq": true })))
    } else {
        Ok(Json(json!({ "requeued": true })))
    }
}

/// POST /heartbeat/:endpoint_name/:message_id — extend visibility timeout
pub async fn heartbeat(
    State(state): State<AppState>,
    Path((_endpoint_name, message_id)): Path<(String, String)>,
    Query(params): Query<HeartbeatParams>,
    req: axum::extract::Request,
) -> Result<impl IntoResponse, HeraldError> {
    let api_key = auth::extract_api_key(&req)?;
    let mut conn = state.redis.clone();
    let _account = auth::lookup_account(&mut conn, &api_key).await?;

    let extend = params.extend.unwrap_or(300).clamp(30, 43200);
    let extended = queue::heartbeat(&mut conn, &message_id, extend).await?;

    if !extended {
        return Err(HeraldError::NotFound(format!(
            "no visibility timeout for {message_id}"
        )));
    }

    Ok(Json(json!({ "visibility_timeout_extended": true })))
}

#[derive(Debug, Deserialize)]
pub struct HeartbeatParams {
    pub extend: Option<u64>,
}
