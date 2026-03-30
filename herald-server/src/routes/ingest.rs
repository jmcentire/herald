use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use base64::Engine as _;
use serde_json::json;

use crate::config::Tier;
use crate::crypto;
use crate::error::HeraldError;
use crate::queue::{self, Message};
use crate::state::AppState;

/// POST /:customer_id/:endpoint_name
/// Inbound webhook ingestion endpoint.
pub async fn ingest_webhook(
    State(state): State<AppState>,
    Path((customer_id, endpoint_name)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<impl IntoResponse, HeraldError> {
    // Validate path components — colons break Redis key structure
    if customer_id.contains(':') || endpoint_name.contains(':') {
        return Err(HeraldError::BadRequest(
            "customer_id and endpoint_name must not contain colons".into(),
        ));
    }

    let mut conn = state.redis.clone();
    let endpoint = format!("{customer_id}/{endpoint_name}");

    // Look up account tier (for now, default to Free if not found)
    let tier = lookup_tier(&mut conn, &customer_id).await;
    let limits = tier.limits();

    // C003: Check payload size
    if body.len() > limits.max_payload_bytes {
        return Err(HeraldError::PayloadTooLarge {
            size: body.len(),
            limit: limits.max_payload_bytes,
        });
    }

    // C003: Check rate limit
    if !queue::check_rate_limit(&mut conn, &customer_id, limits.max_messages_per_day).await? {
        return Err(HeraldError::RateLimited);
    }

    // C004: Check queue depth
    if !queue::check_queue_depth(&mut conn, &customer_id, &endpoint_name, limits.max_queue_depth)
        .await?
    {
        return Err(HeraldError::QueueFull);
    }

    // C005: Compute fingerprint for deduplication
    let fp = crypto::fingerprint(&body);

    // Check dedup
    if queue::check_dedup(&mut conn, &customer_id, &endpoint_name, &fp).await? {
        tracing::info!(
            fingerprint = %fp,
            endpoint = %endpoint,
            "deduplicated at ingestion"
        );
        return Ok((
            StatusCode::OK,
            Json(json!({
                "fingerprint": fp,
                "deduplicated": true,
            })),
        ));
    }

    let received_at = queue::now_nanos();

    // Compute unique message ID
    let message_id = crypto::message_id(&endpoint, received_at, &body);

    // C002: Encrypt body before storage
    let encrypted_body =
        crypto::encrypt(&state.config.service_encryption_key, &body)?;

    // Encrypt headers (service key, even under BYOK — spec decision)
    let headers_json = serialize_headers(&headers);
    let encrypted_headers =
        crypto::encrypt(&state.config.service_encryption_key, headers_json.as_bytes())?;
    let headers_b64 =
        base64::engine::general_purpose::STANDARD.encode(&encrypted_headers);

    let msg = Message {
        message_id: message_id.clone(),
        fingerprint: fp.clone(),
        endpoint: endpoint_name.clone(),
        headers: Some(headers_b64),
        body: encrypted_body,
        encryption: "service".to_string(),
        key_version: None,
        received_at,
        deliver_count: 0,
    };

    let retention_secs = limits.retention.as_secs();
    queue::enqueue(&mut conn, &customer_id, &msg, retention_secs).await?;

    Ok((
        StatusCode::OK,
        Json(json!({
            "message_id": message_id,
            "fingerprint": fp,
            "received_at": received_at.to_string(),
        })),
    ))
}

/// Serialize relevant request headers to JSON.
fn serialize_headers(headers: &HeaderMap) -> String {
    let mut map = serde_json::Map::new();
    for (name, value) in headers.iter() {
        if let Ok(v) = value.to_str() {
            map.insert(name.to_string(), serde_json::Value::String(v.to_string()));
        }
    }
    serde_json::Value::Object(map).to_string()
}

/// Look up the tier for a customer. Falls back to Free if unknown.
async fn lookup_tier(conn: &mut redis::aio::MultiplexedConnection, customer_id: &str) -> Tier {
    let tier_str: Option<String> = redis::cmd("GET")
        .arg(format!("tier:{customer_id}"))
        .query_async(conn)
        .await
        .unwrap_or(None);

    match tier_str.as_deref() {
        Some("standard") => Tier::Standard,
        Some("pro") => Tier::Pro,
        Some("enterprise") => Tier::Enterprise,
        _ => Tier::Free,
    }
}
