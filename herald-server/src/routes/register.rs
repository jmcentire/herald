use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use rand::RngCore;
use serde::Deserialize;
use serde_json::json;

use crate::auth;
use crate::config::Tier;
use crate::error::HeraldError;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct RegisterRequest {
    /// Desired customer ID (e.g., "wander-sync"). Must not contain colons.
    pub customer_id: String,
}

/// POST /register
/// Creates a new account with a generated API key. Returns the key.
/// Idempotent: if the customer_id already has a key, returns it.
pub async fn register(
    State(state): State<AppState>,
    Json(body): Json<RegisterRequest>,
) -> Result<impl IntoResponse, HeraldError> {
    if body.customer_id.is_empty() || body.customer_id.contains(':') {
        return Err(HeraldError::BadRequest(
            "customer_id must be non-empty and must not contain colons".into(),
        ));
    }

    let mut conn = state.redis.clone();

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

    // Register the account (stores apikey:{key} → account JSON)
    auth::register_account(&mut conn, &api_key, &body.customer_id, Tier::Free).await?;

    // Store reverse mapping (customer_id → api_key) for idempotency
    let _: () = redis::cmd("SET")
        .arg(format!("customer_apikey:{}", body.customer_id))
        .arg(&api_key)
        .query_async(&mut conn)
        .await?;

    tracing::info!(
        customer_id = %body.customer_id,
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
