use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use rand::RngCore;
use serde::Deserialize;
use serde_json::json;
use subtle::ConstantTimeEq;

use crate::auth::{self, CustomerConfig, IngestAuth};
use crate::config::Tier;
use crate::error::HeraldError;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct RegisterRequest {
    /// Desired customer ID (e.g., "wander-sync"). Must not contain colons.
    pub customer_id: String,
    /// Optional ingest authentication config for webhook providers.
    #[serde(default)]
    pub ingest_auth: Option<IngestAuth>,
    /// Optional per-customer configuration (encryption mode, retention, etc.).
    #[serde(default)]
    pub config: Option<CustomerConfig>,
}

/// POST /register
/// Creates a new account with a generated API key. Returns the key.
/// Idempotent: if the customer_id already has a key, returns it.
/// If ingest_auth is provided, stores/updates it (even on idempotent calls).
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

    // Parse body manually (can't use Json extractor since we already extracted headers)
    let body: RegisterRequest = serde_json::from_slice(&body)
        .map_err(|e| HeraldError::BadRequest(format!("invalid request body: {e}")))?;

    if body.customer_id.is_empty() || body.customer_id.contains(':') {
        return Err(HeraldError::BadRequest(
            "customer_id must be non-empty and must not contain colons".into(),
        ));
    }

    let mut conn = state.redis.clone();

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
        return Ok((
            StatusCode::OK,
            Json(json!({
                "customer_id": body.customer_id,
                "api_key": key,
                "created": false,
            })),
        ));
    }

    // Generate API key: hrl_sk_{random_hex}
    let mut key_bytes = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut key_bytes);
    let api_key = format!("hrl_sk_{}", hex::encode(key_bytes));

    // Register the account
    auth::register_account(&mut conn, &api_key, &body.customer_id, Tier::Free).await?;

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
            "customer_id": body.customer_id,
            "api_key": api_key,
            "created": true,
        })),
    ))
}
