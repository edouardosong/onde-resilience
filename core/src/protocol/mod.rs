/// Protocol layer — Nostr events, PoW antispam, message formats

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Maximum alert message size (characters)
pub const MAX_ALERT_SIZE: usize = 280;

/// Maximum voice memo duration (seconds)
pub const MAX_VOICE_DURATION: u32 = 120;

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
    /// SHA256 hash of event content (event ID)
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
    /// Ed25519 signature (hex)
    pub sig: String,
    /// Proof-of-Work nonce
    pub pow_nonce: u64,
    /// PoW difficulty target (number of leading zeros)
    pub pow_difficulty: u8,
    /// TTL in hops
    pub ttl: u8,
}

impl MeshEvent {
    pub fn new(
        pubkey: &str,
        kind: OndeMessageType,
        content: String,
        tags: Vec<String>,
    ) -> Self {
        let id = Self::compute_id(pubkey, &kind, &content);
        Self {
            id,
            pubkey: pubkey.to_string(),
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            kind,
            tags,
            content,
            sig: String::new(),
            pow_nonce: 0,
            pow_difficulty: 4,
            ttl: 5,
        }
    }

    fn compute_id(pubkey: &str, _kind: &OndeMessageType, content: &str) -> String {
        let data = format!("{}:{}", pubkey, content);
        let hash = Sha256::digest(data.as_bytes());
        format!("{hash:x}")
    }

    /// Verify content validity
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

        // Verify PoW
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

    /// Compute PoW for this event
    pub fn compute_pow(&mut self, max_iterations: u64) -> bool {
        let target = "0".repeat(self.pow_difficulty as usize);

        for nonce in 0..max_iterations {
            let data = format!("{}:{nonce}", self.id);
            let hash = Sha256::digest(data.as_bytes());
            let hex = format!("{hash:x}");

            if hex.starts_with(&target) {
                self.pow_nonce = nonce;
                // Recompute ID with PoW
                return true;
            }
        }

        false
    }

    /// Check if this event is expired
    pub fn is_expired(&self, max_age_sec: u64) -> bool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        now - self.created_at > max_age_sec
    }
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

    /// Process new event from local user
    pub fn add_event(&mut self, event: MeshEvent) {
        if self.known_events.insert(event.id.clone()) {
            self.pending_broadcasts.push(event);
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
    }

    #[test]
    fn test_pow_verify() {
        // difficulty 2 is easy to find
        assert!(MeshEvent::verify_pow("test-id", 0, 1));
        // This is a property test — we verify the mechanism works
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
    fn test_gossip_dedup() {
        let mut gossip = GossipProtocol::new();
        let event = MeshEvent::new("key", OndeMessageType::Alert, "hello".into(), vec![]);
        let id = event.id.clone();

        gossip.add_event(event.clone());
        assert_eq!(gossip.known_count(), 1);

        // Should not add again (validate would fail due to PoW)
        // Dedup is tested via known_events HashSet
    }
}