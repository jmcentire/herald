use serde::{Deserialize, Serialize};

use crate::error::CliError;

/// Herald API client for polling, ACK, and NACK operations.
pub struct HeraldClient {
    base_url: String,
    api_key: String,
    http: reqwest::Client,
}

#[derive(Debug, Deserialize)]
pub struct PollResponse {
    #[serde(default)]
    pub data: Vec<QueueMessage>,
    #[serde(default)]
    pub has_more: bool,
    #[serde(default)]
    pub queue_depth: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct QueueMessage {
    /// Wire-format message id (e.g. `msg_<hex>`). Treat as opaque.
    pub message_id: String,
    /// Wire-format fingerprint (e.g. `fp_<hex>`). Treat as opaque.
    pub fingerprint: String,
    pub body: String,
    pub headers: Option<serde_json::Value>,
    /// Unix integer seconds since epoch.
    pub received_at: i64,
    pub deliver_count: u32,
    pub encryption: String,
    pub key_version: Option<String>,
}

#[derive(Debug, Serialize)]
struct BatchAckRequest {
    message_ids: Vec<String>,
}

#[derive(Debug, Serialize)]
struct NackBody {
    disposition: &'static str,
}

impl HeraldClient {
    pub fn new(base_url: &str, api_key: &str) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("failed to build HTTP client");

        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: api_key.to_string(),
            http,
        }
    }

    /// Poll for messages from an endpoint.
    pub async fn poll(
        &self,
        endpoint: &str,
        limit: usize,
        visibility_timeout: u64,
    ) -> Result<Vec<QueueMessage>, CliError> {
        let url = format!(
            "{}/endpoints/{}/messages?limit={}&visibility_timeout={}",
            self.base_url, endpoint, limit, visibility_timeout
        );

        let resp = self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()
            .await
            .map_err(|e| CliError::Http(e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(CliError::Http(format!("{status}: {body}")));
        }

        let poll_resp: PollResponse = resp
            .json()
            .await
            .map_err(|e| CliError::Http(e.to_string()))?;

        Ok(poll_resp.data)
    }

    /// Acknowledge a processed message. `message_id` should be the wire-format
    /// id returned by `poll` (e.g. `msg_<hex>`).
    pub async fn ack(&self, endpoint: &str, message_id: &str) -> Result<(), CliError> {
        let url = format!(
            "{}/endpoints/{}/messages/{}/ack",
            self.base_url, endpoint, message_id
        );

        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()
            .await
            .map_err(|e| CliError::Http(e.to_string()))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(CliError::Http(format!("ack failed: {body}")));
        }

        Ok(())
    }

    /// Negative-acknowledge a message. `permanent: true` routes to the DLQ;
    /// `false` requeues for another attempt.
    pub async fn nack(
        &self,
        endpoint: &str,
        message_id: &str,
        permanent: bool,
    ) -> Result<(), CliError> {
        let url = format!(
            "{}/endpoints/{}/messages/{}/nack",
            self.base_url, endpoint, message_id
        );

        let body = NackBody {
            disposition: if permanent { "dlq" } else { "requeue" },
        };

        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&body)
            .send()
            .await
            .map_err(|e| CliError::Http(e.to_string()))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(CliError::Http(format!("nack failed: {body}")));
        }

        Ok(())
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn api_key(&self) -> &str {
        &self.api_key
    }
}
