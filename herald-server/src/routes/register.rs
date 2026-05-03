use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use rand::RngCore;
use redis::AsyncCommands;
use serde::Deserialize;
use serde_json::json;
use subtle::ConstantTimeEq;

use crate::auth::{self, CustomerConfig, IngestAuth};
use crate::config::Tier;
use crate::error::HeraldError;
use crate::queue;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct RegisterRequest {
    /// Desired customer ID. Must match `^[a-z0-9][a-z0-9-]{2,31}$` and not
    /// collide with a reserved top-level path segment.
    pub customer_id: String,
    /// Optional ingest authentication config for webhook providers.
    #[serde(default)]
    pub ingest_auth: Option<IngestAuth>,
    /// Optional per-customer configuration (encryption mode, retention, etc.).
    #[serde(default)]
    pub config: Option<CustomerConfig>,
}

/// Reserved customer_ids that would collide with top-level routes or look
/// like service identifiers. Kept lowercase; matched after validation.
const RESERVED_CUSTOMER_IDS: &[&str] = &[
    "account", "ack", "admin", "api", "billing", "dlq", "docs", "endpoints",
    "health", "heartbeat", "login", "logout", "messages", "nack", "queue",
    "register", "root", "stream", "stripe", "system", "www",
];

/// Per-IP rate limits for /register (defends against bulk credential generation).
const REGISTER_BURST_PER_MIN: u64 = 5;
const REGISTER_DAILY_LIMIT: u64 = 50;

/// POST /register
/// Creates a new account with a generated API key. Returns the key.
/// Idempotent: if the customer_id already has a key, returns it (HTTP 200).
/// Brand-new accounts return HTTP 201.
///
/// If HERALD_REGISTER_SECRET is set, requires Authorization: Bearer {secret}.
pub async fn register(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<impl IntoResponse, HeraldError> {
    // Guard: if register_secret is configured, require matching Bearer token
    if let Some(ref expected) = state.config.register_secret {
        let provided = auth::extract_bearer_from_headers(&headers)
            .map_err(|_| HeraldError::Unauthorized("register requires authorization".into()))?;
        if !bool::from(provided.as_bytes().ct_eq(expected.as_bytes())) {
            return Err(HeraldError::Unauthorized(
                "invalid register secret".into(),
            ));
        }
    }

    let mut conn = state.redis.clone();

    // Per-IP rate limit. Falls back to a shared "unknown" bucket if neither
    // X-Forwarded-For nor X-Real-IP is set (deployments fronted by a proxy
    // should set one of these).
    let ip = client_ip(&headers).unwrap_or_else(|| "unknown".into());
    enforce_register_rate_limit(&mut conn, &ip).await?;

    // Parse body manually (can't use Json extractor since we already extracted headers)
    let body: RegisterRequest = serde_json::from_slice(&body)
        .map_err(|e| HeraldError::BadRequest(format!("invalid request body: {e}")))?;

    validate_customer_id(&body.customer_id)?;

    // Store/update ingest auth if provided (do this for both new and existing accounts)
    if let Some(ref ingest_auth) = body.ingest_auth {
        auth::store_ingest_auth(
            &mut conn,
            &body.customer_id,
            ingest_auth,
            &state.config.service_encryption_key,
        )
        .await?;
    }

    // Store/update customer config if provided
    if let Some(ref config) = body.config {
        auth::store_customer_config(&mut conn, &body.customer_id, config).await?;
    }

    // Check if customer already has a key
    let existing_key: Option<String> = redis::cmd("GET")
        .arg(format!("customer_apikey:{}", body.customer_id))
        .query_async(&mut conn)
        .await?;

    if let Some(key) = existing_key {
        // Look up the account to surface its real created_at.
        let account = auth::lookup_account(&mut conn, &key).await?;
        return Ok((
            StatusCode::OK,
            Json(json!({
                "object": "account",
                "customer_id": body.customer_id,
                "api_key": key,
                "created": account.created_at,
            })),
        ));
    }

    // Generate API key: hrl_sk_{random_hex}
    let mut key_bytes = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut key_bytes);
    let api_key = format!("hrl_sk_{}", hex::encode(key_bytes));

    let created_at = queue::now_seconds();

    // Register the account
    auth::register_account(&mut conn, &api_key, &body.customer_id, Tier::Free, created_at)
        .await?;

    // Store reverse mapping for idempotency
    let _: () = redis::cmd("SET")
        .arg(format!("customer_apikey:{}", body.customer_id))
        .arg(&api_key)
        .query_async(&mut conn)
        .await?;

    tracing::info!(
        customer_id = %body.customer_id,
        has_ingest_auth = body.ingest_auth.is_some(),
        "registered new account"
    );

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "object": "account",
            "customer_id": body.customer_id,
            "api_key": api_key,
            "created": created_at,
        })),
    ))
}

fn validate_customer_id(id: &str) -> Result<(), HeraldError> {
    let len = id.len();
    if !(3..=32).contains(&len) {
        return Err(HeraldError::BadRequest(
            "customer_id must be 3-32 characters".into(),
        ));
    }

    let bytes = id.as_bytes();
    let first = bytes[0];
    if !matches!(first, b'a'..=b'z' | b'0'..=b'9') {
        return Err(HeraldError::BadRequest(
            "customer_id must start with a lowercase letter or digit".into(),
        ));
    }
    for b in bytes {
        if !matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'-') {
            return Err(HeraldError::BadRequest(
                "customer_id may contain only lowercase letters, digits, and hyphens".into(),
            ));
        }
    }

    if RESERVED_CUSTOMER_IDS.contains(&id) {
        return Err(HeraldError::BadRequest(format!(
            "customer_id '{id}' is reserved"
        )));
    }

    Ok(())
}

fn client_ip(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            headers
                .get("x-real-ip")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
}

async fn enforce_register_rate_limit(
    conn: &mut redis::aio::MultiplexedConnection,
    ip: &str,
) -> Result<(), HeraldError> {
    let burst_key = format!("register_burst:{ip}");
    let daily_key = format!("register_daily:{ip}");

    let burst_count: u64 = conn.incr(&burst_key, 1u64).await?;
    if burst_count == 1 {
        let _: () = conn.expire(&burst_key, 60).await?;
    }
    if burst_count > REGISTER_BURST_PER_MIN {
        return Err(HeraldError::RateLimited);
    }

    let daily_count: u64 = conn.incr(&daily_key, 1u64).await?;
    if daily_count == 1 {
        let _: () = conn.expire(&daily_key, 86400).await?;
    }
    if daily_count > REGISTER_DAILY_LIMIT {
        return Err(HeraldError::RateLimited);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_customer_id_accepts_simple() {
        assert!(validate_customer_id("my-agent").is_ok());
        assert!(validate_customer_id("agent42").is_ok());
        assert!(validate_customer_id("abc").is_ok());
    }

    #[test]
    fn test_validate_customer_id_rejects_too_short() {
        assert!(validate_customer_id("ab").is_err());
    }

    #[test]
    fn test_validate_customer_id_rejects_too_long() {
        let long = "a".repeat(33);
        assert!(validate_customer_id(&long).is_err());
    }

    #[test]
    fn test_validate_customer_id_rejects_uppercase() {
        assert!(validate_customer_id("MyAgent").is_err());
    }

    #[test]
    fn test_validate_customer_id_rejects_underscore() {
        assert!(validate_customer_id("my_agent").is_err());
    }

    #[test]
    fn test_validate_customer_id_rejects_leading_hyphen() {
        assert!(validate_customer_id("-agent").is_err());
    }

    #[test]
    fn test_validate_customer_id_rejects_colon() {
        assert!(validate_customer_id("agent:1").is_err());
    }

    #[test]
    fn test_validate_customer_id_rejects_reserved() {
        for &name in RESERVED_CUSTOMER_IDS {
            assert!(
                validate_customer_id(name).is_err(),
                "expected {name} to be rejected"
            );
        }
    }

    #[test]
    fn test_client_ip_prefers_xff() {
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", "203.0.113.5, 10.0.0.1".parse().unwrap());
        h.insert("x-real-ip", "10.0.0.1".parse().unwrap());
        assert_eq!(client_ip(&h).as_deref(), Some("203.0.113.5"));
    }

    #[test]
    fn test_client_ip_falls_back_to_xri() {
        let mut h = HeaderMap::new();
        h.insert("x-real-ip", "203.0.113.7".parse().unwrap());
        assert_eq!(client_ip(&h).as_deref(), Some("203.0.113.7"));
    }

    #[test]
    fn test_client_ip_none_when_missing() {
        let h = HeaderMap::new();
        assert!(client_ip(&h).is_none());
    }
}
