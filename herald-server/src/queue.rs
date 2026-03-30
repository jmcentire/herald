use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};

use crate::error::HeraldError;

/// A message stored in the queue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub message_id: String,
    pub fingerprint: String,
    pub endpoint: String,
    pub headers: Option<String>,
    pub body: Vec<u8>,
    pub encryption: String,
    pub key_version: Option<String>,
    pub received_at: u128,
    pub deliver_count: u32,
}

/// Keys follow the Redis data model from the spec:
///   queue:{customer_id}:{endpoint}      - main FIFO
///   inflight:{customer_id}:{endpoint}   - popped but not ACKed
///   dead:{customer_id}:{endpoint}       - DLQ
///   meta:{message_id}                   - message metadata hash
///   dedup:{customer_id}:{endpoint}      - fingerprint set
///   rate:{customer_id}                  - rate limit counter
fn queue_key(customer_id: &str, endpoint: &str) -> String {
    format!("queue:{customer_id}:{endpoint}")
}

fn inflight_key(customer_id: &str, endpoint: &str) -> String {
    format!("inflight:{customer_id}:{endpoint}")
}

fn dead_key(customer_id: &str, endpoint: &str) -> String {
    format!("dead:{customer_id}:{endpoint}")
}

fn meta_key(message_id: &str) -> String {
    format!("meta:{message_id}")
}

fn dedup_key(customer_id: &str, endpoint: &str) -> String {
    format!("dedup:{customer_id}:{endpoint}")
}

fn visibility_key(message_id: &str) -> String {
    format!("visibility:{message_id}")
}

fn rate_key(customer_id: &str) -> String {
    format!("rate:{customer_id}")
}

fn notify_channel(customer_id: &str, endpoint: &str) -> String {
    format!("notify:{customer_id}:{endpoint}")
}

/// Check if a fingerprint already exists (deduplication).
pub async fn check_dedup(
    conn: &mut redis::aio::MultiplexedConnection,
    customer_id: &str,
    endpoint: &str,
    fp: &str,
) -> Result<bool, HeraldError> {
    let exists: bool = conn.sismember(dedup_key(customer_id, endpoint), fp).await?;
    Ok(exists)
}

/// Check and increment rate limit. Returns true if within limit.
pub async fn check_rate_limit(
    conn: &mut redis::aio::MultiplexedConnection,
    customer_id: &str,
    max_per_day: u64,
) -> Result<bool, HeraldError> {
    let key = rate_key(customer_id);
    let count: u64 = conn.incr(&key, 1u64).await?;
    if count == 1 {
        // First request today — set expiry to 24 hours
        let _: () = conn.expire(&key, 86400).await?;
    }
    Ok(count <= max_per_day)
}

/// Check queue depth against limit.
pub async fn check_queue_depth(
    conn: &mut redis::aio::MultiplexedConnection,
    customer_id: &str,
    endpoint: &str,
    max_depth: u64,
) -> Result<bool, HeraldError> {
    let depth: u64 = conn.llen(queue_key(customer_id, endpoint)).await?;
    Ok(depth < max_depth)
}

/// Enqueue a message. Returns true if enqueued, false if deduplicated.
pub async fn enqueue(
    conn: &mut redis::aio::MultiplexedConnection,
    customer_id: &str,
    msg: &Message,
    retention_secs: u64,
) -> Result<bool, HeraldError> {
    let dk = dedup_key(customer_id, &msg.endpoint);
    let qk = queue_key(customer_id, &msg.endpoint);
    let mk = meta_key(&msg.message_id);

    // Check dedup
    let already_exists: bool = conn.sismember(&dk, &msg.fingerprint).await?;
    if already_exists {
        tracing::info!(
            message_id = %msg.message_id,
            fingerprint = %msg.fingerprint,
            "deduplicated"
        );
        return Ok(false);
    }

    // Store message metadata and enqueue atomically via pipeline
    let body_b64 = base64::engine::general_purpose::STANDARD.encode(&msg.body);
    let headers_val = msg.headers.as_deref().unwrap_or("");
    let key_version_val = msg.key_version.as_deref().unwrap_or("");
    let nc = notify_channel(customer_id, &msg.endpoint);

    let fields: Vec<(&str, String)> = vec![
        ("message_id", msg.message_id.clone()),
        ("fingerprint", msg.fingerprint.clone()),
        ("endpoint", msg.endpoint.clone()),
        ("headers", headers_val.to_string()),
        ("body", body_b64),
        ("encryption", msg.encryption.clone()),
        ("key_version", key_version_val.to_string()),
        ("received_at", msg.received_at.to_string()),
        ("deliver_count", msg.deliver_count.to_string()),
    ];

    // Pipeline: SADD (dedup) + HSET (meta) + EXPIRE (TTL) + LPUSH (queue) + PUBLISH (notify)
    let _: () = redis::pipe()
        .atomic()
        .cmd("SADD").arg(&dk).arg(&msg.fingerprint).ignore()
        .cmd("HSET").arg(&mk).arg(&fields).ignore()
        .cmd("EXPIRE").arg(&mk).arg(retention_secs as i64).ignore()
        .cmd("LPUSH").arg(&qk).arg(&msg.message_id).ignore()
        .cmd("PUBLISH").arg(&nc).arg(&msg.message_id).ignore()
        .query_async(conn)
        .await?;

    tracing::info!(
        message_id = %msg.message_id,
        fingerprint = %msg.fingerprint,
        endpoint = %msg.endpoint,
        "enqueued"
    );

    Ok(true)
}

/// Fetch messages from the queue (polling).
/// Moves messages from queue to inflight atomically.
pub async fn fetch(
    conn: &mut redis::aio::MultiplexedConnection,
    customer_id: &str,
    endpoint: &str,
    limit: usize,
    visibility_timeout_secs: u64,
) -> Result<Vec<Message>, HeraldError> {
    let qk = queue_key(customer_id, endpoint);
    let ik = inflight_key(customer_id, endpoint);
    let mut messages = Vec::with_capacity(limit);

    for _ in 0..limit {
        // RPOPLPUSH: atomically move from queue tail to inflight head
        let msg_id: Option<String> = redis::cmd("RPOPLPUSH")
            .arg(&qk)
            .arg(&ik)
            .query_async(conn)
            .await?;

        let msg_id = match msg_id {
            Some(id) => id,
            None => break, // Queue empty
        };

        // Set visibility timeout
        let vk = visibility_key(&msg_id);
        let _: () = conn.set_ex(&vk, "1", visibility_timeout_secs).await?;

        // Increment deliver count
        let mk = meta_key(&msg_id);
        let _: () = redis::cmd("HINCRBY")
            .arg(&mk)
            .arg("deliver_count")
            .arg(1)
            .query_async(conn)
            .await?;

        // Fetch message metadata
        let fields: HashMap<String, String> = conn.hgetall(&mk).await?;
        if fields.is_empty() {
            tracing::warn!(message_id = %msg_id, "message metadata missing, skipping");
            continue;
        }

        let body_b64 = fields.get("body").cloned().unwrap_or_default();
        let body =
            base64::engine::general_purpose::STANDARD.decode(&body_b64)
                .unwrap_or_default();

        let headers = fields.get("headers").cloned().filter(|h| !h.is_empty());
        let key_version = fields
            .get("key_version")
            .cloned()
            .filter(|v| !v.is_empty());

        let received_at: u128 = fields
            .get("received_at")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        let deliver_count: u32 = fields
            .get("deliver_count")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        messages.push(Message {
            message_id: msg_id,
            fingerprint: fields.get("fingerprint").cloned().unwrap_or_default(),
            endpoint: endpoint.to_string(),
            headers,
            body,
            encryption: fields.get("encryption").cloned().unwrap_or_default(),
            key_version,
            received_at,
            deliver_count,
        });
    }

    tracing::info!(
        customer_id = %customer_id,
        endpoint = %endpoint,
        count = messages.len(),
        "fetched messages"
    );

    Ok(messages)
}

/// Acknowledge a message — remove from inflight, delete metadata.
pub async fn ack(
    conn: &mut redis::aio::MultiplexedConnection,
    customer_id: &str,
    endpoint: &str,
    message_id: &str,
) -> Result<bool, HeraldError> {
    let ik = inflight_key(customer_id, endpoint);
    let mk = meta_key(message_id);
    let vk = visibility_key(message_id);

    let removed: u32 = conn.lrem(&ik, 1, message_id).await?;
    if removed == 0 {
        return Ok(false);
    }

    let _: () = conn.del(&vk).await?;
    let _: () = conn.del(&mk).await?;

    tracing::info!(message_id = %message_id, "acknowledged");
    Ok(true)
}

/// Negative-acknowledge a message — move back to queue or to DLQ.
pub async fn nack(
    conn: &mut redis::aio::MultiplexedConnection,
    customer_id: &str,
    endpoint: &str,
    message_id: &str,
    permanent: bool,
    max_retries: u32,
) -> Result<bool, HeraldError> {
    let ik = inflight_key(customer_id, endpoint);
    let vk = visibility_key(message_id);

    let removed: u32 = conn.lrem(&ik, 1, message_id).await?;
    if removed == 0 {
        return Ok(false);
    }

    let _: () = conn.del(&vk).await?;

    if permanent {
        move_to_dlq(conn, customer_id, endpoint, message_id).await?;
        return Ok(true);
    }

    // Check deliver count
    let mk = meta_key(message_id);
    let deliver_count: u32 = redis::cmd("HGET")
        .arg(&mk)
        .arg("deliver_count")
        .query_async(conn)
        .await
        .unwrap_or(0);

    if deliver_count >= max_retries {
        move_to_dlq(conn, customer_id, endpoint, message_id).await?;
    } else {
        // Requeue at the tail (will be fetched last = back of FIFO)
        let qk = queue_key(customer_id, endpoint);
        let _: () = conn.rpush(&qk, message_id).await?;
        tracing::info!(message_id = %message_id, deliver_count, "requeued");
    }

    Ok(true)
}

/// Move a message to the dead letter queue.
async fn move_to_dlq(
    conn: &mut redis::aio::MultiplexedConnection,
    customer_id: &str,
    endpoint: &str,
    message_id: &str,
) -> Result<(), HeraldError> {
    let dk = dead_key(customer_id, endpoint);
    let _: () = conn.lpush(&dk, message_id).await?;
    tracing::warn!(message_id = %message_id, "moved to DLQ");
    Ok(())
}

/// Extend visibility timeout for a message (heartbeat).
pub async fn heartbeat(
    conn: &mut redis::aio::MultiplexedConnection,
    message_id: &str,
    extend_secs: u64,
) -> Result<bool, HeraldError> {
    let vk = visibility_key(message_id);
    let exists: bool = conn.exists(&vk).await?;
    if !exists {
        return Ok(false);
    }
    let _: () = conn.expire(&vk, extend_secs as i64).await?;
    Ok(true)
}

/// Reaper: find expired visibility keys and requeue their messages.
/// This should be called periodically (e.g., every 10 seconds).
pub async fn reap_expired(
    conn: &mut redis::aio::MultiplexedConnection,
    max_retries: u32,
) -> Result<u32, HeraldError> {
    // Scan inflight:* keys for messages whose visibility key has expired
    let mut cursor: u64 = 0;
    let mut reaped: u32 = 0;

    loop {
        let (new_cursor, keys): (u64, Vec<String>) = redis::cmd("SCAN")
            .arg(cursor)
            .arg("MATCH")
            .arg("inflight:*")
            .arg("COUNT")
            .arg(100)
            .query_async(conn)
            .await?;

        for inflight_list_key in &keys {
            // Parse customer_id and endpoint from key
            let parts: Vec<&str> = inflight_list_key.splitn(3, ':').collect();
            if parts.len() != 3 {
                continue;
            }
            let customer_id = parts[1];
            let endpoint = parts[2];

            // Check each message in the inflight list
            let msg_ids: Vec<String> = conn.lrange(inflight_list_key, 0, -1).await?;
            for msg_id in &msg_ids {
                let vk = visibility_key(msg_id);
                let exists: bool = conn.exists(&vk).await?;
                if !exists {
                    // Visibility expired — nack it
                    tracing::info!(message_id = %msg_id, "visibility expired, reaping");
                    nack(conn, customer_id, endpoint, msg_id, false, max_retries).await?;
                    reaped += 1;
                }
            }
        }

        cursor = new_cursor;
        if cursor == 0 {
            break;
        }
    }

    if reaped > 0 {
        tracing::info!(reaped, "reaper cycle complete");
    }

    Ok(reaped)
}

/// Get the current time in nanoseconds since epoch.
pub fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_nanos()
}
