use axum::extract::Request;
use axum::http::header::AUTHORIZATION;

use crate::config::Tier;
use crate::error::HeraldError;

/// Represents an authenticated account.
#[derive(Debug, Clone)]
pub struct Account {
    pub customer_id: String,
    pub tier: Tier,
    pub api_key: String,
}

/// Extract API key from Authorization header (Bearer token).
pub fn extract_api_key(req: &Request) -> Result<String, HeraldError> {
    let header = req
        .headers()
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
/// In production, this would query Redis for the key → account mapping.
/// For now, we use a simple key format: hrl_sk_{customer_id}_{tier}
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

/// Register an account in Redis (for testing / bootstrapping).
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
        // Convert to axum::extract::Request type for the function
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
}
