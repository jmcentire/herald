use axum::extract::Request;
use axum::http::header::AUTHORIZATION;
use axum::http::HeaderMap;
use base64::Engine as _;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use subtle::ConstantTimeEq;

use crate::config::Tier;
use crate::crypto;
use crate::error::HeraldError;

// =============================================================================
// Account
// =============================================================================

/// Represents an authenticated account.
#[derive(Debug, Clone)]
pub struct Account {
    pub customer_id: String,
    pub tier: Tier,
    pub api_key: String,
}

/// Extract API key from Authorization header (Bearer token).
pub fn extract_api_key(req: &Request) -> Result<String, HeraldError> {
    extract_bearer_from_headers(req.headers())
}

/// Extract Bearer token from headers.
pub fn extract_bearer_from_headers(headers: &HeaderMap) -> Result<String, HeraldError> {
    let header = headers
        .get(AUTHORIZATION)
        .ok_or_else(|| HeraldError::Unauthorized("missing Authorization header".into()))?;

    let value = header
        .to_str()
        .map_err(|_| HeraldError::Unauthorized("invalid Authorization header".into()))?;

    let key = value
        .strip_prefix("Bearer ")
        .ok_or_else(|| HeraldError::Unauthorized("expected Bearer token".into()))?;

    if key.is_empty() {
        return Err(HeraldError::Unauthorized("empty API key".into()));
    }

    Ok(key.to_string())
}

/// Look up an account by API key from Redis.
pub async fn lookup_account(
    conn: &mut redis::aio::MultiplexedConnection,
    api_key: &str,
) -> Result<Account, HeraldError> {
    let account_json: Option<String> = redis::cmd("GET")
        .arg(format!("apikey:{api_key}"))
        .query_async(conn)
        .await?;

    match account_json {
        Some(json) => {
            let val: serde_json::Value =
                serde_json::from_str(&json).map_err(|e| HeraldError::Internal(e.to_string()))?;

            let customer_id = val["customer_id"]
                .as_str()
                .ok_or_else(|| HeraldError::Internal("missing customer_id".into()))?
                .to_string();

            let tier = match val["tier"].as_str().unwrap_or("free") {
                "standard" => Tier::Standard,
                "pro" => Tier::Pro,
                "enterprise" => Tier::Enterprise,
                _ => Tier::Free,
            };

            Ok(Account {
                customer_id,
                tier,
                api_key: api_key.to_string(),
            })
        }
        None => Err(HeraldError::Unauthorized("invalid API key".into())),
    }
}

/// Register an account in Redis.
pub async fn register_account(
    conn: &mut redis::aio::MultiplexedConnection,
    api_key: &str,
    customer_id: &str,
    tier: Tier,
) -> Result<(), HeraldError> {
    let tier_str = match tier {
        Tier::Free => "free",
        Tier::Standard => "standard",
        Tier::Pro => "pro",
        Tier::Enterprise => "enterprise",
    };

    let json = serde_json::json!({
        "customer_id": customer_id,
        "tier": tier_str,
    });

    let _: () = redis::cmd("SET")
        .arg(format!("apikey:{api_key}"))
        .arg(json.to_string())
        .query_async(conn)
        .await?;

    Ok(())
}

// =============================================================================
// Ingest Authentication
// =============================================================================

/// Per-customer ingest authentication configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IngestAuth {
    /// Shared secret — provider sends Authorization: Bearer {secret}
    Bearer { secret: String },
    /// HMAC-SHA256 signature — provider signs body, puts signature in named header
    Hmac { key: String, header: String },
}

/// Store ingest auth config in Redis (encrypted).
pub async fn store_ingest_auth(
    conn: &mut redis::aio::MultiplexedConnection,
    customer_id: &str,
    auth: &IngestAuth,
    encryption_key: &[u8; 32],
) -> Result<(), HeraldError> {
    let json = serde_json::to_vec(auth)
        .map_err(|e| HeraldError::Internal(format!("serialize ingest_auth: {e}")))?;
    let encrypted = crypto::encrypt(encryption_key, &json)?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(&encrypted);

    let _: () = redis::cmd("SET")
        .arg(format!("ingest_auth:{customer_id}"))
        .arg(encoded)
        .query_async(conn)
        .await?;

    Ok(())
}

/// Load ingest auth config from Redis (decrypt). Returns None if not configured.
pub async fn load_ingest_auth(
    conn: &mut redis::aio::MultiplexedConnection,
    customer_id: &str,
    encryption_key: &[u8; 32],
) -> Result<Option<IngestAuth>, HeraldError> {
    let encoded: Option<String> = redis::cmd("GET")
        .arg(format!("ingest_auth:{customer_id}"))
        .query_async(conn)
        .await?;

    let Some(encoded) = encoded else {
        return Ok(None);
    };

    let encrypted = base64::engine::general_purpose::STANDARD
        .decode(&encoded)
        .map_err(|e| HeraldError::Internal(format!("decode ingest_auth: {e}")))?;

    let json = crypto::decrypt(encryption_key, &encrypted)?;

    let auth: IngestAuth = serde_json::from_slice(&json)
        .map_err(|e| HeraldError::Internal(format!("deserialize ingest_auth: {e}")))?;

    Ok(Some(auth))
}

/// Validate ingest request against configured auth.
pub fn validate_ingest_auth(
    auth: &IngestAuth,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<(), HeraldError> {
    match auth {
        IngestAuth::Bearer { secret } => validate_bearer(secret, headers),
        IngestAuth::Hmac { key, header } => validate_hmac(key, header, headers, body),
    }
}

/// Validate Bearer token (constant-time comparison).
fn validate_bearer(expected_secret: &str, headers: &HeaderMap) -> Result<(), HeraldError> {
    let provided = extract_bearer_from_headers(headers)?;
    if provided.as_bytes().ct_eq(expected_secret.as_bytes()).into() {
        Ok(())
    } else {
        Err(HeraldError::Unauthorized("invalid ingest secret".into()))
    }
}

/// Validate HMAC-SHA256 signature.
fn validate_hmac(
    key: &str,
    header_name: &str,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<(), HeraldError> {
    let sig_header = headers
        .get(header_name)
        .ok_or_else(|| {
            HeraldError::Unauthorized(format!("missing signature header: {header_name}"))
        })?
        .to_str()
        .map_err(|_| HeraldError::Unauthorized("invalid signature header value".into()))?;

    // Strip common prefixes (GitHub: "sha256=", some providers: "sha256:")
    let sig_value = sig_header
        .strip_prefix("sha256=")
        .or_else(|| sig_header.strip_prefix("sha256:"))
        .unwrap_or(sig_header);

    // Try hex decode, then base64
    let sig_bytes = hex::decode(sig_value)
        .or_else(|_| base64::engine::general_purpose::STANDARD.decode(sig_value))
        .map_err(|_| HeraldError::Unauthorized("could not decode signature".into()))?;

    // Compute expected HMAC
    let mut mac = Hmac::<Sha256>::new_from_slice(key.as_bytes())
        .map_err(|e| HeraldError::Internal(format!("hmac init: {e}")))?;
    mac.update(body);

    // Constant-time verify
    mac.verify_slice(&sig_bytes)
        .map_err(|_| HeraldError::Unauthorized("HMAC signature mismatch".into()))?;

    Ok(())
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request as HttpRequest;

    #[test]
    fn test_extract_api_key_valid() {
        let req = HttpRequest::builder()
            .header("Authorization", "Bearer hrl_sk_test123")
            .body(())
            .unwrap();
        let req = Request::from_parts(req.into_parts().0, axum::body::Body::empty());
        let key = extract_api_key(&req).unwrap();
        assert_eq!(key, "hrl_sk_test123");
    }

    #[test]
    fn test_extract_api_key_missing_header() {
        let req = HttpRequest::builder().body(()).unwrap();
        let req = Request::from_parts(req.into_parts().0, axum::body::Body::empty());
        assert!(extract_api_key(&req).is_err());
    }

    #[test]
    fn test_extract_api_key_wrong_scheme() {
        let req = HttpRequest::builder()
            .header("Authorization", "Basic dXNlcjpwYXNz")
            .body(())
            .unwrap();
        let req = Request::from_parts(req.into_parts().0, axum::body::Body::empty());
        assert!(extract_api_key(&req).is_err());
    }

    #[test]
    fn test_validate_bearer_correct() {
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, "Bearer my-secret".parse().unwrap());
        assert!(validate_bearer("my-secret", &headers).is_ok());
    }

    #[test]
    fn test_validate_bearer_wrong() {
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, "Bearer wrong".parse().unwrap());
        assert!(validate_bearer("my-secret", &headers).is_err());
    }

    #[test]
    fn test_validate_bearer_missing() {
        let headers = HeaderMap::new();
        assert!(validate_bearer("my-secret", &headers).is_err());
    }

    #[test]
    fn test_validate_hmac_hex() {
        let key = "test-signing-key";
        let body = b"hello world";

        // Compute expected signature
        let mut mac = Hmac::<Sha256>::new_from_slice(key.as_bytes()).unwrap();
        mac.update(body);
        let sig = hex::encode(mac.finalize().into_bytes());

        let mut headers = HeaderMap::new();
        headers.insert("X-Signature", sig.parse().unwrap());
        assert!(validate_hmac(key, "X-Signature", &headers, body).is_ok());
    }

    #[test]
    fn test_validate_hmac_with_prefix() {
        let key = "test-key";
        let body = b"payload";

        let mut mac = Hmac::<Sha256>::new_from_slice(key.as_bytes()).unwrap();
        mac.update(body);
        let sig = format!("sha256={}", hex::encode(mac.finalize().into_bytes()));

        let mut headers = HeaderMap::new();
        headers.insert("X-Hub-Signature-256", sig.parse().unwrap());
        assert!(validate_hmac(key, "X-Hub-Signature-256", &headers, body).is_ok());
    }

    #[test]
    fn test_validate_hmac_wrong_key() {
        let body = b"hello";

        let mut mac = Hmac::<Sha256>::new_from_slice(b"correct-key").unwrap();
        mac.update(body);
        let sig = hex::encode(mac.finalize().into_bytes());

        let mut headers = HeaderMap::new();
        headers.insert("X-Sig", sig.parse().unwrap());
        assert!(validate_hmac("wrong-key", "X-Sig", &headers, body).is_err());
    }

    #[test]
    fn test_validate_hmac_tampered_body() {
        let key = "key";

        let mut mac = Hmac::<Sha256>::new_from_slice(key.as_bytes()).unwrap();
        mac.update(b"original body");
        let sig = hex::encode(mac.finalize().into_bytes());

        let mut headers = HeaderMap::new();
        headers.insert("X-Sig", sig.parse().unwrap());
        assert!(validate_hmac(key, "X-Sig", &headers, b"tampered body").is_err());
    }

    #[test]
    fn test_ingest_auth_serde_roundtrip_bearer() {
        let auth = IngestAuth::Bearer {
            secret: "s3cret".into(),
        };
        let json = serde_json::to_string(&auth).unwrap();
        let parsed: IngestAuth = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, IngestAuth::Bearer { secret } if secret == "s3cret"));
    }

    #[test]
    fn test_ingest_auth_serde_roundtrip_hmac() {
        let auth = IngestAuth::Hmac {
            key: "mykey".into(),
            header: "X-Signature".into(),
        };
        let json = serde_json::to_string(&auth).unwrap();
        let parsed: IngestAuth = serde_json::from_str(&json).unwrap();
        assert!(
            matches!(parsed, IngestAuth::Hmac { key, header } if key == "mykey" && header == "X-Signature")
        );
    }
}
