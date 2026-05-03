use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use rand::RngCore;
use sha2::{Digest, Sha256};

use crate::error::HeraldError;

/// Compute SHA-256 fingerprint of raw bytes (for deduplication).
pub fn fingerprint(data: &[u8]) -> String {
    let hash = Sha256::digest(data);
    hex::encode(hash)
}

/// Compute unique message ID from endpoint + timestamp + body.
pub fn message_id(endpoint: &str, received_at_nanos: u128, body: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(endpoint.as_bytes());
    hasher.update(received_at_nanos.to_le_bytes());
    hasher.update(body);
    hex::encode(hasher.finalize())
}

pub const MESSAGE_ID_PREFIX: &str = "msg_";
pub const FINGERPRINT_PREFIX: &str = "fp_";

/// Wrap a raw hex message_id for the wire (e.g. "msg_abc...").
pub fn wire_message_id(raw_hex: &str) -> String {
    format!("{MESSAGE_ID_PREFIX}{raw_hex}")
}

/// Wrap a raw hex fingerprint for the wire (e.g. "fp_abc...").
pub fn wire_fingerprint(raw_hex: &str) -> String {
    format!("{FINGERPRINT_PREFIX}{raw_hex}")
}

/// Strip the `msg_` prefix from a wire-format message_id.
/// Returns Err if the prefix is missing or the suffix is not lowercase hex of length 64.
pub fn parse_wire_message_id(s: &str) -> Result<String, HeraldError> {
    parse_wire_id(MESSAGE_ID_PREFIX, s)
}

/// Strip the `fp_` prefix from a wire-format fingerprint.
pub fn parse_wire_fingerprint(s: &str) -> Result<String, HeraldError> {
    parse_wire_id(FINGERPRINT_PREFIX, s)
}

fn parse_wire_id(prefix: &str, s: &str) -> Result<String, HeraldError> {
    let raw = s.strip_prefix(prefix).ok_or_else(|| {
        HeraldError::BadRequest(format!("expected id with prefix '{prefix}'"))
    })?;
    if raw.len() != 64 || !raw.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return Err(HeraldError::BadRequest(
            "id suffix must be 64 lowercase hex characters".into(),
        ));
    }
    Ok(raw.to_string())
}

/// Encrypt data with AES-256-GCM using the provided key.
/// Returns nonce (12 bytes) prepended to ciphertext.
pub fn encrypt(key: &[u8; 32], plaintext: &[u8]) -> Result<Vec<u8>, HeraldError> {
    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|e| HeraldError::Encryption(format!("key init: {e}")))?;

    let mut nonce_bytes = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| HeraldError::Encryption(format!("encrypt: {e}")))?;

    let mut result = Vec::with_capacity(12 + ciphertext.len());
    result.extend_from_slice(&nonce_bytes);
    result.extend_from_slice(&ciphertext);
    Ok(result)
}

/// Decrypt data encrypted with `encrypt`. Expects nonce prepended to ciphertext.
pub fn decrypt(key: &[u8; 32], data: &[u8]) -> Result<Vec<u8>, HeraldError> {
    if data.len() < 12 {
        return Err(HeraldError::Encryption("ciphertext too short".into()));
    }

    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|e| HeraldError::Encryption(format!("key init: {e}")))?;

    let nonce = Nonce::from_slice(&data[..12]);
    let ciphertext = &data[12..];

    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| HeraldError::Encryption(format!("decrypt: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fingerprint_deterministic() {
        let data = b"hello world";
        assert_eq!(fingerprint(data), fingerprint(data));
    }

    #[test]
    fn test_fingerprint_different_data() {
        assert_ne!(fingerprint(b"hello"), fingerprint(b"world"));
    }

    #[test]
    fn test_message_id_unique_with_timestamp() {
        let id1 = message_id("ep1", 1000, b"body");
        let id2 = message_id("ep1", 2000, b"body");
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_message_id_unique_with_endpoint() {
        let id1 = message_id("ep1", 1000, b"body");
        let id2 = message_id("ep2", 1000, b"body");
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let key = [42u8; 32];
        let plaintext = b"sensitive webhook payload";
        let encrypted = encrypt(&key, plaintext).unwrap();
        let decrypted = decrypt(&key, &encrypted).unwrap();
        assert_eq!(&decrypted, plaintext);
    }

    #[test]
    fn test_encrypt_produces_different_ciphertext() {
        let key = [42u8; 32];
        let plaintext = b"same data";
        let enc1 = encrypt(&key, plaintext).unwrap();
        let enc2 = encrypt(&key, plaintext).unwrap();
        // Different nonces should produce different ciphertext
        assert_ne!(enc1, enc2);
        // But both decrypt to the same plaintext
        assert_eq!(decrypt(&key, &enc1).unwrap(), plaintext);
        assert_eq!(decrypt(&key, &enc2).unwrap(), plaintext);
    }

    #[test]
    fn test_decrypt_tampered_data_fails() {
        let key = [42u8; 32];
        let mut encrypted = encrypt(&key, b"data").unwrap();
        // Tamper with the ciphertext
        let last = encrypted.len() - 1;
        encrypted[last] ^= 0xFF;
        assert!(decrypt(&key, &encrypted).is_err());
    }

    #[test]
    fn test_decrypt_short_data_fails() {
        let key = [42u8; 32];
        assert!(decrypt(&key, &[0u8; 5]).is_err());
    }

    #[test]
    fn test_wire_message_id_roundtrip() {
        let raw = fingerprint(b"hello world"); // 64-char hex
        let wire = wire_message_id(&raw);
        assert!(wire.starts_with("msg_"));
        assert_eq!(parse_wire_message_id(&wire).unwrap(), raw);
    }

    #[test]
    fn test_wire_fingerprint_roundtrip() {
        let raw = fingerprint(b"hello world");
        let wire = wire_fingerprint(&raw);
        assert!(wire.starts_with("fp_"));
        assert_eq!(parse_wire_fingerprint(&wire).unwrap(), raw);
    }

    #[test]
    fn test_parse_wire_id_rejects_missing_prefix() {
        let raw = fingerprint(b"x");
        assert!(parse_wire_message_id(&raw).is_err());
    }

    #[test]
    fn test_parse_wire_id_rejects_wrong_prefix() {
        let raw = fingerprint(b"x");
        assert!(parse_wire_message_id(&format!("fp_{raw}")).is_err());
    }

    #[test]
    fn test_parse_wire_id_rejects_short_suffix() {
        assert!(parse_wire_message_id("msg_deadbeef").is_err());
    }

    #[test]
    fn test_parse_wire_id_rejects_non_hex() {
        let bad = "z".repeat(64);
        assert!(parse_wire_message_id(&format!("msg_{bad}")).is_err());
    }

    #[test]
    fn test_raw_byte_preservation() {
        // C010: Herald must preserve exact raw bytes through encrypt/decrypt
        let key = [42u8; 32];
        let raw_bytes: Vec<u8> = (0..=255).collect();
        let encrypted = encrypt(&key, &raw_bytes).unwrap();
        let decrypted = decrypt(&key, &encrypted).unwrap();
        assert_eq!(decrypted, raw_bytes, "raw bytes must survive encryption roundtrip");
    }
}
