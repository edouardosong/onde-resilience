/// Protocol layer — Nostr events, PoW antispam, message formats

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::crypto::Identity;

/// Maximum alert message size (characters)
pub const MAX_ALERT_SIZE: usize = 280;

/// Maximum voice memo duration (seconds)
pub const MAX_VOICE_DURATION: u32 = 120;

/// Maximum acceptable clock skew for event timestamps (5 minutes)
pub const MAX_CLOCK_SKEW_SECS: u64 = 300;

/// Current UNIX timestamp in seconds
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/*
 * Nostr-style Event System
 */

/// ONDE message type
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum OndeMessageType {
    /// Public alert (280 chars max, no images)
    Alert,
    /// Mutual aid request (hierarchical)
    MutualAid,
    /// Async voice memo (Opus 8kbps)
    VoiceMemo,
    /// Voice-to-text transcription
    Transcription,
    /// ZK transaction
    Transaction,
    /// AI query
    AiQuery,
    /// AI response
    AiResponse,
    /// P2P file share request
    FileShareRequest,
    /// Heartbeat / status
    Heartbeat,
}

/// Nostr-style event for the mesh network
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeshEvent {
    /// SHA256 hash of the canonical event serialization (event ID)
    pub id: String,
    /// Public key of creator (hex)
    pub pubkey: String,
    /// Unix timestamp
    pub created_at: u64,
    /// Event kind
    pub kind: OndeMessageType,
    /// Tags for routing/filtering
    pub tags: Vec<String>,
    /// Content (text or base64 data)
    pub content: String,
    /// Ed25519 signature over the canonical ID (hex)
    pub sig: String,
    /// Proof-of-Work nonce
    pub pow_nonce: u64,
    /// PoW difficulty target (number of leading zeros)
    pub pow_difficulty: u8,
    /// TTL in hops
    pub ttl: u8,
}

impl MeshEvent {
    /// Create an UNSIGNED event (sig is empty → rejected by `validate`).
    /// Prefer `new_signed` for events that must pass validation.
    pub fn new(
        pubkey: &str,
        kind: OndeMessageType,
        content: String,
        tags: Vec<String>,
    ) -> Self {
        let created_at = now_secs();
        let id = Self::compute_id(pubkey, created_at, &kind, &tags, &content);
        Self {
            id,
            pubkey: pubkey.to_string(),
            created_at,
            kind,
            tags,
            content,
            sig: String::new(), // unsigned → rejected by validate()
            pow_nonce: 0,
            pow_difficulty: 4,
            ttl: 5,
        }
    }

    /// Create an event signed with the sender's Ed25519 identity.
    /// The canonical ID is computed first, then signed.
    pub fn new_signed(
        sender: &Identity,
        kind: OndeMessageType,
        content: String,
        tags: Vec<String>,
    ) -> Self {
        let pubkey = sender.pubkey_hex();
        let created_at = now_secs();
        let id = Self::compute_id(&pubkey, created_at, &kind, &tags, &content);
        let sig = hex::encode(sender.sign(id.as_bytes()));
        Self {
            id,
            pubkey,
            created_at,
            kind,
            tags,
            content,
            sig,
            pow_nonce: 0,
            pow_difficulty: 4,
            ttl: 5,
        }
    }

    /// Canonical SHA-256 event ID over the immutable fields:
    /// `[pubkey, created_at, kind as u8, tags (stable order), content]`.
    /// All fields are part of the serialization, so any difference in
    /// created_at or kind yields a different ID.
    fn compute_id(
        pubkey: &str,
        created_at: u64,
        kind: &OndeMessageType,
        tags: &[String],
        content: &str,
    ) -> String {
        let canonical = serde_json::json!([
            pubkey,
            created_at,
            Self::kind_code(kind),
            tags,
            content
        ]);
        let data = serde_json::to_vec(&canonical).unwrap();
        hex::encode(Sha256::digest(&data))
    }

    /// Stable numeric code for an event kind
    fn kind_code(kind: &OndeMessageType) -> u8 {
        match kind {
            OndeMessageType::Alert => 0,
            OndeMessageType::MutualAid => 1,
            OndeMessageType::VoiceMemo => 2,
            OndeMessageType::Transcription => 3,
            OndeMessageType::Transaction => 4,
            OndeMessageType::AiQuery => 5,
            OndeMessageType::AiResponse => 6,
            OndeMessageType::FileShareRequest => 7,
            OndeMessageType::Heartbeat => 8,
        }
    }

    /// Verify content validity: size limit, timestamp sanity, canonical ID,
    /// Ed25519 signature and PoW.
    pub fn validate(&self) -> Result<(), String> {
        // Alert size limit
        if let OndeMessageType::Alert = &self.kind {
            if self.content.len() > MAX_ALERT_SIZE {
                return Err(format!(
                    "Alert exceeds {} character limit",
                    MAX_ALERT_SIZE
                ));
            }
        }

        // Reject timestamps too far in the future (clock skew > 5 minutes)
        let now = now_secs();
        if self.created_at > now && self.created_at - now > MAX_CLOCK_SKEW_SECS {
            return Err("Event created_at is too far in the future".to_string());
        }

        // The ID must match the canonical serialization of the fields
        let expected_id = Self::compute_id(
            &self.pubkey,
            self.created_at,
            &self.kind,
            &self.tags,
            &self.content,
        );
        if self.id != expected_id {
            return Err("Event ID does not match canonical serialization".to_string());
        }

        // Signature must be present and valid (Ed25519 over the canonical ID)
        if self.sig.is_empty() {
            return Err("Missing signature".to_string());
        }
        let pubkey_bytes = decode_hex_32(&self.pubkey)
            .map_err(|_| "Invalid pubkey encoding".to_string())?;
        let sig_bytes = decode_hex_64(&self.sig)
            .map_err(|_| "Invalid signature encoding".to_string())?;
        if !Identity::verify_from_pubkey(&pubkey_bytes, self.id.as_bytes(), &sig_bytes) {
            return Err("Invalid signature".to_string());
        }

        // Verify PoW (the ID is stable — PoW is checked against hash(id:nonce))
        if !Self::verify_pow(&self.id, self.pow_nonce, self.pow_difficulty) {
            return Err("Invalid PoW".to_string());
        }

        Ok(())
    }

    /// Verify proof of work
    fn verify_pow(event_id: &str, nonce: u64, difficulty: u8) -> bool {
        let data = format!("{event_id}:{nonce}");
        let hash = Sha256::digest(data.as_bytes());
        let hex = format!("{hash:x}");
        let zeros = difficulty;
        hex.starts_with(&"0".repeat(zeros as usize))
    }

    /// Compute PoW for this event.
    ///
    /// The ID is STABLE — PoW never changes it. The nonce is stored separately
    /// and verified via hash(id:nonce), so the canonical ID and signature stay
    /// valid regardless of PoW.
    pub fn compute_pow(&mut self, max_iterations: u64) -> bool {
        let target = "0".repeat(self.pow_difficulty as usize);

        for nonce in 0..max_iterations {
            let data = format!("{}:{nonce}", self.id);
            let hash = Sha256::digest(data.as_bytes());
            let hex = format!("{hash:x}");

            if hex.starts_with(&target) {
                self.pow_nonce = nonce;
                return true;
            }
        }

        false
    }

    /// Check if this event is expired (saturating — never underflows)
    pub fn is_expired(&self, max_age_sec: u64) -> bool {
        let now = now_secs();
        now.saturating_sub(self.created_at) > max_age_sec
    }
}

/// Decode a 32-byte hex string
fn decode_hex_32(s: &str) -> Result<[u8; 32], ()> {
    let bytes = hex::decode(s).map_err(|_| ())?;
    if bytes.len() != 32 {
        return Err(());
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
}

/// Decode a 64-byte hex string
fn decode_hex_64(s: &str) -> Result<[u8; 64], ()> {
    let bytes = hex::decode(s).map_err(|_| ())?;
    if bytes.len() != 64 {
        return Err(());
    }
    let mut out = [0u8; 64];
    out.copy_from_slice(&bytes);
    Ok(out)
}

/*
 * Gossip Protocol for Public Feed
 */

/// Gossip protocol state
pub struct GossipProtocol {
    known_events: std::collections::HashSet<String>,
    pending_broadcasts: Vec<MeshEvent>,
    peer_cache: std::collections::HashMap<String, Vec<String>>,
}

impl GossipProtocol {
    pub fn new() -> Self {
        Self {
            known_events: std::collections::HashSet::new(),
            pending_broadcasts: Vec::new(),
            peer_cache: std::collections::HashMap::new(),
        }
    }

    /// Process a new event from the local user.
    ///
    /// The event is validated first — invalid events are refused.
    /// Returns `Ok(true)` if added, `Ok(false)` if already known,
    /// `Err(reason)` if the event is invalid.
    pub fn add_event(&mut self, event: MeshEvent) -> Result<bool, String> {
        event.validate()?;
        if self.known_events.insert(event.id.clone()) {
            self.pending_broadcasts.push(event);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Process event received from peer
    pub fn receive_event(&mut self, event: MeshEvent, peer_id: &str) -> bool {
        if self.known_events.contains(&event.id) {
            return false; // Duplicate
        }

        if event.validate().is_ok() {
            self.known_events.insert(event.id.clone());
            self.pending_broadcasts.push(event);
            true
        } else {
            false
        }
    }

    /// Get events to broadcast to peer
    pub fn get_pending_for_peer(&mut self, _peer_id: &str) -> Vec<MeshEvent> {
        self.pending_broadcasts.drain(..).collect()
    }

    pub fn known_count(&self) -> usize {
        self.known_events.len()
    }

    /// Get pending broadcasts
    pub fn get_pending_broadcasts(&self) -> Vec<&MeshEvent> {
        self.pending_broadcasts.iter().collect()
    }
}

/*
 * Voice Memo — Opus Codec Wrapper
 */

/// Encapsulated voice memo
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceMemo {
    pub event_id: String,
    /// Opus encoded audio at 8kbps
    pub opus_data: Vec<u8>,
    /// Duration in seconds
    pub duration_sec: f32,
    /// Auto-transcribed text (filled at receive)
    pub transcription: Option<String>,
}

impl VoiceMemo {
    pub fn new(event_id: String, opus_data: Vec<u8>, duration_sec: f32) -> Self {
        Self {
            event_id,
            opus_data,
            duration_sec,
            transcription: None,
        }
    }

    /// Estimated size at 8kbps (1000 bytes/sec)
    pub fn estimated_size_bytes(&self) -> usize {
        (self.duration_sec * 1000.0) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_creation() {
        let event = MeshEvent::new(
            "pubkey123",
            OndeMessageType::Alert,
            "Test alert".to_string(),
            vec![],
        );
        assert_eq!(event.pubkey, "pubkey123");
        assert!(event.pow_nonce == 0); // Not computed yet
        // Unsigned events are not valid
        assert!(event.sig.is_empty());
    }

    #[test]
    fn test_pow_verify() {
        // difficulty 2 is easy to find
        assert!(MeshEvent::verify_pow("test-id", 0, 1));
    }

    #[test]
    fn test_alert_size_limit() {
        let event = MeshEvent::new(
            "key",
            OndeMessageType::Alert,
            "x".repeat(MAX_ALERT_SIZE + 1),
            vec![],
        );
        assert!(event.validate().is_err());
    }

    #[test]
    fn test_signed_event_validates() {
        let identity = Identity::generate();
        let mut event = MeshEvent::new_signed(
            &identity,
            OndeMessageType::Alert,
            "hello".to_string(),
            vec!["test".to_string()],
        );
        event.pow_difficulty = 0; // PoW trivially satisfied for this unit test

        assert!(event.validate().is_ok(), "Signed event must validate");
        assert_eq!(event.sig.len(), 128); // 64 bytes of hex
        assert_eq!(event.pubkey, identity.pubkey_hex());
    }

    #[test]
    fn test_tampered_signature_rejected() {
        let identity = Identity::generate();
        let mut event = MeshEvent::new_signed(
            &identity,
            OndeMessageType::Alert,
            "hello".to_string(),
            vec![],
        );
        event.pow_difficulty = 0;
        assert!(event.validate().is_ok());

        // Falsify the signature (flip one bit)
        let mut sig_bytes = hex::decode(&event.sig).unwrap();
        sig_bytes[0] ^= 0x01;
        event.sig = hex::encode(&sig_bytes);

        assert!(
            event.validate().is_err(),
            "Event with a falsified signature must be rejected"
        );
    }

    #[test]
    fn test_unsigned_event_rejected() {
        // new() leaves the signature empty → validate() must refuse
        let event = MeshEvent::new(
            "key",
            OndeMessageType::Alert,
            "hello".to_string(),
            vec![],
        );
        assert!(
            event.validate().is_err(),
            "Event with an empty signature must be rejected"
        );
    }

    #[test]
    fn test_tampered_content_rejected() {
        let identity = Identity::generate();
        let mut event = MeshEvent::new_signed(
            &identity,
            OndeMessageType::Alert,
            "hello".to_string(),
            vec![],
        );
        event.pow_difficulty = 0;

        // Tamper with content — the canonical ID no longer matches
        event.content = "tampered".to_string();
        assert!(
            event.validate().is_err(),
            "Event whose content does not match its ID must be rejected"
        );
    }

    #[test]
    fn test_canonical_id_distinct() {
        let pk = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef";
        let kind = OndeMessageType::Alert;
        let tags = vec!["a".to_string()];
        let content = "same content";

        // Same author + content, created_at shifted by 1s → different IDs
        let id_t = MeshEvent::compute_id(pk, 1_000, &kind, &tags, content);
        let id_t_plus_1 = MeshEvent::compute_id(pk, 1_001, &kind, &tags, content);
        assert_ne!(
            id_t, id_t_plus_1,
            "created_at +1s must yield a different ID"
        );

        // Same author + content + timestamp, different kind → different IDs
        let id_alert = MeshEvent::compute_id(pk, 1_000, &OndeMessageType::Alert, &tags, content);
        let id_voice = MeshEvent::compute_id(pk, 1_000, &OndeMessageType::VoiceMemo, &tags, content);
        assert_ne!(id_alert, id_voice, "different kinds must yield different IDs");

        // Determinism: identical inputs → identical ID
        let id_again = MeshEvent::compute_id(pk, 1_000, &kind, &tags, content);
        assert_eq!(id_t, id_again, "canonical ID must be deterministic");
    }

    #[test]
    fn test_is_expired_no_underflow() {
        let identity = Identity::generate();
        let mut event = MeshEvent::new_signed(&identity, OndeMessageType::Alert, "hi".into(), vec![]);
        event.pow_difficulty = 0;

        // An event created in the future must not panic (saturating_sub)
        event.created_at = now_secs() + 60;
        assert!(!event.is_expired(3600));

        // Old event is expired
        event.created_at = now_secs() - 7200;
        assert!(event.is_expired(3600));
    }

    #[test]
    fn test_future_timestamp_rejected() {
        let identity = Identity::generate();
        let mut event = MeshEvent::new_signed(&identity, OndeMessageType::Alert, "hi".into(), vec![]);
        event.pow_difficulty = 0;
        assert!(event.validate().is_ok());

        // created_at more than 5 minutes in the future → refused
        event.created_at = now_secs() + MAX_CLOCK_SKEW_SECS + 1;
        assert!(
            event.validate().is_err(),
            "Event created too far in the future must be rejected"
        );
    }

    #[test]
    fn test_gossip_dedup() {
        let mut gossip = GossipProtocol::new();
        let identity = Identity::generate();
        let mut event = MeshEvent::new_signed(&identity, OndeMessageType::Alert, "hello".into(), vec![]);
        event.pow_difficulty = 0;
        let id = event.id.clone();

        assert!(gossip.add_event(event.clone()).is_ok());
        assert_eq!(gossip.known_count(), 1);
        assert_eq!(gossip.get_pending_broadcasts().len(), 1);

        // Duplicate is not added again
        assert!(!gossip.add_event(event.clone()).unwrap());
        assert_eq!(gossip.known_count(), 1);
        assert_eq!(gossip.get_pending_broadcasts().len(), 1);
        assert_eq!(gossip.get_pending_broadcasts()[0].id, id);

        // Invalid (unsigned) event is refused by add_event
        let bad = MeshEvent::new("key", OndeMessageType::Alert, "hello".into(), vec![]);
        assert!(
            gossip.add_event(bad).is_err(),
            "Invalid event must be refused by add_event"
        );
    }
}
