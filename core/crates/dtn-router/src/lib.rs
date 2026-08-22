//! DTN Router — Store-and-Forward Delay Tolerant Network
//!
//! Handles message buffering, opportunistic forwarding,
//! and delivery when end-to-end paths don't exist.

use std::collections::{HashMap, VecDeque};
use tokio::sync::Mutex;

/// A message in the DTN network
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DtnMessage {
    pub id: String,
    pub sender: String,
    pub destination: Option<String>, // None = broadcast
    pub payload: Vec<u8>,
    pub msg_type: MessageType,
    pub ttl: u8,
    pub hop_count: u8,
    pub timestamp_ms: u64,
    pub priority: u8, // 0=highest
    /// Peers this message has already been delivered to (broadcast dedup).
    /// Empty for unicast. `#[serde(default)]` keeps older serialized messages
    /// (without this field) decodable.
    #[serde(default)]
    pub delivered_to: Vec<String>,
}

impl DtnMessage {
    /// Parse a `DtnMessage` from arbitrary JSON bytes (the unreliable DTN wire
    /// feed). Returns `Err` on malformed JSON or wrong shape — never panics, so
    /// it is safe as a cargo-fuzz entry point.
    pub fn from_wire_bytes(bytes: &[u8]) -> Result<DtnMessage, String> {
        serde_json::from_slice(bytes).map_err(|e| format!("dtn wire parse failed: {e}"))
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum MessageType {
    Alert,
    MutualAid,
    Voice,
    Transaction,
    AiQuery,
    AiResponse,
}

/// DTN Router state
pub struct DtnRouter {
    /// Buffer per node: node_id -> queue of messages
    buffers: Mutex<HashMap<String, VecDeque<DtnMessage>>>,
    /// Max buffer size per node
    max_buffer: usize,
    /// Stats
    stats: Mutex<RouterStats>,
}

#[derive(Debug, Default, Clone)]
pub struct RouterStats {
    pub total_stored: u64,
    pub total_forwarded: u64,
    pub total_expired: u64,
    pub total_dropped: u64,
    pub total_delivered: u64,
}

impl DtnRouter {
    pub fn new(max_buffer: usize) -> Self {
        Self {
            buffers: Mutex::new(HashMap::new()),
            max_buffer,
            stats: Mutex::new(RouterStats::default()),
        }
    }

    /// Store a message in this node's buffer.
    ///
    /// Returns `true` if the message was stored, `false` if it was rejected.
    ///
    /// Buffer policy when full (documented):
    /// priority convention is `0 = most urgent`, larger numbers = less urgent.
    /// If the buffer is full, the least urgent stored message (highest priority
    /// number; ties broken by oldest `timestamp_ms`) is dropped ONLY if the
    /// incoming message is strictly more urgent (smaller priority number).
    /// If the incoming message is not more urgent, it is rejected and the
    /// buffer is left unchanged.
    pub async fn store(&self, node_id: &str, msg: DtnMessage) -> bool {
        let mut replaced = false;
        let mut rejected = false;
        {
            let mut buffers = self.buffers.lock().await;
            let buf = buffers.entry(node_id.to_string()).or_default();

            if buf.len() >= self.max_buffer {
                if let Some(idx) = worst_index(buf) {
                    let worst_prio = buf[idx].priority;
                    if msg.priority < worst_prio {
                        // Incoming message is more urgent: drop the least
                        // urgent stored one and make room.
                        buf.remove(idx);
                        replaced = true;
                    } else {
                        // Incoming message is not more urgent: reject it.
                        rejected = true;
                    }
                }
                // max_buffer == 0: never "full", store anyway
            }
            if !rejected {
                buf.push_back(msg);
            }
        } // buffers guard released — no `.await` inside the critical section

        let mut stats = self.stats.lock().await;
        if replaced || rejected {
            stats.total_dropped += 1;
        }
        if !rejected {
            stats.total_stored += 1;
        }
        !rejected
    }

    /// Opportunistic forward when two nodes encounter each other
    pub async fn encounter(
        &self,
        node_a: &str,
        node_b: &str,
    ) -> (Vec<DtnMessage>, Vec<DtnMessage>) {
        let (to_a, to_b, forwarded, delivered, expired) = {
            // All buffer mutation happens in the synchronous helper below —
            // no `.await` while holding the `buffers` guard.
            let mut buffers = self.buffers.lock().await;
            let (to_a, fwd_a, del_a, exp_a) = collect_forwarded(&mut buffers, node_b, node_a);
            let (to_b, fwd_b, del_b, exp_b) = collect_forwarded(&mut buffers, node_a, node_b);
            (to_a, to_b, fwd_a + fwd_b, del_a + del_b, exp_a + exp_b)
        }; // buffers guard released

        let mut stats = self.stats.lock().await;
        stats.total_forwarded += forwarded;
        stats.total_delivered += delivered;
        stats.total_expired += expired;

        (to_a, to_b)
    }

    /// Get buffer size for a node
    pub async fn buffer_size(&self, node_id: &str) -> usize {
        let buffers = self.buffers.lock().await;
        buffers.get(node_id).map(|b| b.len()).unwrap_or(0)
    }

    /// Get router stats
    pub async fn stats(&self) -> RouterStats {
        self.stats.lock().await.clone()
    }

    /// Decrement TTL and expire old messages
    pub async fn tick(&self, node_id: &str) -> Vec<DtnMessage> {
        // Collect expired messages first
        let (expired, expired_count) = {
            let mut buffers = self.buffers.lock().await;
            let mut expired = Vec::new();
            let mut expired_count: u64 = 0;

            if let Some(buf) = buffers.get_mut(node_id) {
                let mut to_remove = Vec::new();
                for (idx, msg) in buf.iter().enumerate() {
                    let new_ttl = msg.ttl.saturating_sub(1);
                    if new_ttl == 0 {
                        expired_count += 1;
                        expired.push(msg.clone());
                        to_remove.push(idx);
                    }
                }
                // Remove expired (reverse order)
                for idx in to_remove.into_iter().rev() {
                    buf.remove(idx);
                }
                // Update remaining TTL
                for msg in buf.iter_mut() {
                    msg.ttl = msg.ttl.saturating_sub(1);
                }
            }
            (expired, expired_count)
        }; // buffers guard released

        // Update stats separately
        if expired_count > 0 {
            let mut stats = self.stats.lock().await;
            stats.total_expired += expired_count;
        }

        expired
    }
}

/// Add `peer` to `list` only if it is not already present (no duplicates).
fn push_unique(list: &mut Vec<String>, peer: &str) {
    if !list.iter().any(|p| p == peer) {
        list.push(peer.to_string());
    }
}

/// Collect messages from `from`'s buffer that should be handed to `to`.
///
/// - Unicast (`destination = Some(to)`): delivered, then removed from the buffer.
/// - Broadcast (`destination = None`): delivered to every peer NOT already in
///   `delivered_to`, and KEPT in the buffer so other peers can still receive it.
///   It only dies by TTL (`tick`) or hop limit — never after a single delivery.
///   The previous holder of the buffer is also marked served so the broadcast
///   is never bounced straight back to it (loopback).
/// - Messages whose hop count reached their TTL are dropped as expired.
///
/// Returns (forwarded messages, forwarded count, delivered count, expired count).
fn collect_forwarded(
    buffers: &mut HashMap<String, VecDeque<DtnMessage>>,
    from: &str,
    to: &str,
) -> (Vec<DtnMessage>, u64, u64, u64) {
    let mut forwarded = Vec::new();
    let mut forwarded_count = 0u64;
    let mut delivered_count = 0u64;
    let mut expired_count = 0u64;

    if let Some(buf) = buffers.get_mut(from) {
        let mut to_remove = Vec::new();
        for (idx, msg) in buf.iter_mut().enumerate() {
            if msg.hop_count >= msg.ttl {
                // Hop limit reached — the message dies here
                expired_count += 1;
                to_remove.push(idx);
                continue;
            }
            let should_deliver = match &msg.destination {
                Some(dest) => dest == to,
                // Broadcast: deliver unless this peer was already served
                None => !msg.delivered_to.iter().any(|p| p == to),
            };
            if should_deliver {
                let mut deliverable = msg.clone();
                deliverable.hop_count += 1;
                // Mark both this peer (`to`) and the buffer holder (`from`) as
                // served so the broadcast is never bounced straight back to the
                // holder on a later encounter (loopback).
                push_unique(&mut deliverable.delivered_to, to);
                if from != to {
                    push_unique(&mut deliverable.delivered_to, from);
                }
                forwarded_count += 1;
                delivered_count += 1;
                forwarded.push(deliverable);
                if msg.destination.is_none() {
                    // Broadcast: keep in the buffer for other peers, mark this peer
                    // and the holder served
                    push_unique(&mut msg.delivered_to, to);
                    if from != to {
                        push_unique(&mut msg.delivered_to, from);
                    }
                } else {
                    // Unicast: delivered, remove from the buffer
                    to_remove.push(idx);
                }
            }
        }
        for idx in to_remove.into_iter().rev() {
            buf.remove(idx);
        }
    }

    (forwarded, forwarded_count, delivered_count, expired_count)
}

/// Index of the least urgent stored message (highest priority number = least
/// urgent, since `0` is the most urgent). Ties are broken by age: the oldest
/// message (smallest `timestamp_ms`) is considered the worst.
fn worst_index(buf: &VecDeque<DtnMessage>) -> Option<usize> {
    let mut worst: Option<(usize, u8, u64)> = None;
    for (i, msg) in buf.iter().enumerate() {
        let is_worse = match worst {
            None => true,
            Some((_, w_prio, w_ts)) => {
                msg.priority > w_prio || (msg.priority == w_prio && msg.timestamp_ms < w_ts)
            }
        };
        if is_worse {
            worst = Some((i, msg.priority, msg.timestamp_ms));
        }
    }
    worst.map(|(idx, _, _)| idx)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(id: &str, dest: Option<&str>, priority: u8, timestamp_ms: u64) -> DtnMessage {
        DtnMessage {
            id: id.into(),
            sender: "A".into(),
            destination: dest.map(|s| s.to_string()),
            payload: b"payload".to_vec(),
            msg_type: MessageType::Alert,
            ttl: 5,
            hop_count: 0,
            timestamp_ms,
            priority,
            delivered_to: vec![],
        }
    }

    #[tokio::test]
    async fn test_store_and_forward() {
        let router = DtnRouter::new(100);

        let msg = msg("test-1", Some("B"), 5, 0);

        assert!(router.store("A", msg).await);
        assert_eq!(router.buffer_size("A").await, 1);

        // Encounter: A meets B, message should forward
        let (to_a, to_b) = router.encounter("A", "B").await;
        assert_eq!(to_a.len(), 0);
        assert_eq!(to_b.len(), 1);
        assert_eq!(to_b[0].id, "test-1");
        // Unicast is delivered once and removed from the buffer
        assert_eq!(router.buffer_size("A").await, 0);
    }

    #[tokio::test]
    async fn test_broadcast_delivered_to_every_peer() {
        let router = DtnRouter::new(100);

        let broadcast = msg("bcast-1", None, 0, 0);
        assert!(router.store("A", broadcast).await);

        // A meets B → B receives, A KEEPS the broadcast
        let (to_a, to_b) = router.encounter("A", "B").await;
        assert_eq!(to_a.len(), 0);
        assert_eq!(to_b.len(), 1, "B must receive the broadcast");
        assert_eq!(to_b[0].id, "bcast-1");
        assert_eq!(
            router.buffer_size("A").await,
            1,
            "A must keep the broadcast"
        );

        // A meets C → C also receives
        let (_, to_c) = router.encounter("A", "C").await;
        assert_eq!(to_c.len(), 1, "C must also receive the broadcast");
        assert_eq!(to_c[0].id, "bcast-1");
        assert_eq!(router.buffer_size("A").await, 1);

        // A meets B again → B already served, must NOT receive it again
        let (_, to_b_again) = router.encounter("A", "B").await;
        assert_eq!(
            to_b_again.len(),
            0,
            "B must not receive the broadcast twice"
        );
        assert_eq!(router.buffer_size("A").await, 1);
    }

    #[tokio::test]
    async fn test_broadcast_dies_by_ttl_not_delivery() {
        let router = DtnRouter::new(100);

        let broadcast = msg("bcast-ttl", None, 0, 0);
        router.store("A", broadcast).await;

        // Deliver to several peers — the broadcast survives
        router.encounter("A", "B").await;
        router.encounter("A", "C").await;
        router.encounter("A", "D").await;
        assert_eq!(
            router.buffer_size("A").await,
            1,
            "deliveries must not remove the broadcast"
        );

        // ttl = 5 → after 5 ticks the broadcast expires
        for _ in 0..5 {
            router.tick("A").await;
        }
        assert_eq!(
            router.buffer_size("A").await,
            0,
            "broadcast must die by TTL"
        );
        assert_eq!(router.stats().await.total_expired, 1);
    }

    #[tokio::test]
    async fn test_broadcast_no_loopback_to_holder() {
        // A originates a broadcast, B receives and stores it, then B encounters
        // A again. Because the buffer holder (`A`) is marked served in
        // `delivered_to`, A must NOT receive its own broadcast bounced back.
        let router = DtnRouter::new(100);

        let broadcast = msg("bcast-loop", None, 0, 0);
        assert!(router.store("A", broadcast).await);

        // A meets B → B receives the broadcast
        let (_, to_b) = router.encounter("A", "B").await;
        assert_eq!(to_b.len(), 1, "B must receive the broadcast");
        // The delivered copy must mark both the new peer (B) and the holder (A)
        assert!(
            to_b[0].delivered_to.iter().any(|p| p == "B"),
            "B marked served"
        );
        assert!(
            to_b[0].delivered_to.iter().any(|p| p == "A"),
            "holder A marked served"
        );

        // B stores the received copy in its own buffer
        assert!(router.store("B", to_b[0].clone()).await);

        // B meets A again → A must NOT receive the broadcast back (loopback)
        let (to_a, _) = router.encounter("B", "A").await;
        assert_eq!(
            to_a.len(),
            0,
            "A must not receive its own broadcast back from B (loopback)"
        );

        // A still holds its own original copy
        assert_eq!(
            router.buffer_size("A").await,
            1,
            "A keeps its own broadcast copy"
        );
    }

    #[tokio::test]
    async fn test_buffer_full_rejects_not_more_urgent() {
        let router = DtnRouter::new(3);

        // Fill the buffer with priority-0 messages
        for i in 0..3 {
            let m = msg(&format!("m{i}"), Some("B"), 0, i as u64);
            assert!(router.store("A", m).await);
        }
        assert_eq!(router.buffer_size("A").await, 3);

        // Incoming message is NOT more urgent (equal priority) → rejected
        let incoming = msg("new", Some("B"), 0, 99);
        assert!(
            !router.store("A", incoming).await,
            "equal-priority message must be rejected when the buffer is full"
        );

        // Buffer unchanged, one message rejected
        assert_eq!(router.buffer_size("A").await, 3);
        assert_eq!(router.stats().await.total_dropped, 1);
    }

    #[tokio::test]
    async fn test_buffer_full_replaces_least_urgent() {
        let router = DtnRouter::new(3);

        // Buffer holds priorities 1, 2, 3 (3 = least urgent)
        for (id, prio) in [("p1", 1u8), ("p2", 2u8), ("p3", 3u8)] {
            let m = msg(id, Some("B"), prio, 0);
            assert!(router.store("A", m).await);
        }
        assert_eq!(router.buffer_size("A").await, 3);

        // Incoming priority 0 (most urgent) → the least urgent (p3) is dropped
        let incoming = msg("urgent", Some("B"), 0, 99);
        assert!(
            router.store("A", incoming).await,
            "more urgent message must replace the least urgent one"
        );

        let buffers = router.buffers.lock().await;
        let buf = buffers.get("A").unwrap();
        let ids: Vec<&str> = buf.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(buf.len(), 3);
        assert!(ids.contains(&"urgent"), "incoming message must be stored");
        assert!(
            !ids.contains(&"p3"),
            "least urgent message (p3) must be dropped"
        );
        assert_eq!(router.stats().await.total_dropped, 1);
    }

    #[tokio::test]
    async fn test_buffer_full_tie_break_drops_oldest() {
        let router = DtnRouter::new(2);

        // Two messages with the same priority, different ages
        let old = msg("old", Some("B"), 1, 100);
        let newer = msg("newer", Some("B"), 1, 200);
        assert!(router.store("A", old).await);
        assert!(router.store("A", newer).await);
        assert_eq!(router.buffer_size("A").await, 2);

        // Incoming is more urgent → drops the worst existing message.
        // Both stored have priority 1 → tie-break drops the OLDEST (timestamp 100).
        let incoming = msg("urgent", Some("B"), 0, 300);
        assert!(router.store("A", incoming).await);
        assert_eq!(router.buffer_size("A").await, 2);

        let buffers = router.buffers.lock().await;
        let buf = buffers.get("A").unwrap();
        let ids: Vec<&str> = buf.iter().map(|m| m.id.as_str()).collect();
        assert!(ids.contains(&"urgent"));
        assert!(
            !ids.contains(&"old"),
            "oldest equal-priority message must be dropped"
        );
        assert!(ids.contains(&"newer"));
    }
}

/// Regression tests for the `from_wire_bytes` cargo-fuzz entry point.
#[cfg(test)]
mod fuzz_regression_tests {
    use super::*;

    #[test]
    fn from_wire_bytes_roundtrip() {
        let msg = DtnMessage {
            id: "a".to_string(),
            sender: "b".to_string(),
            destination: None,
            payload: vec![1u8, 2, 3],
            msg_type: MessageType::Alert,
            ttl: 5,
            hop_count: 1,
            timestamp_ms: 0,
            priority: 0,
            delivered_to: vec![],
        };
        let wire = serde_json::to_vec(&msg).unwrap();
        let back = DtnMessage::from_wire_bytes(&wire).unwrap();
        assert_eq!(back.id, "a");
        assert_eq!(back.payload, vec![1u8, 2, 3]);
    }

    #[test]
    fn from_wire_bytes_rejects_garbage_no_panic() {
        // Arbitrary bytes must return a Result, never panic (fuzz safety).
        for _ in 0..5 {
            let chunk: Vec<u8> = (0..16u8).collect();
            let _ = DtnMessage::from_wire_bytes(&chunk);
        }
        // Empty and whitespace-only inputs are tolerated as Result too.
        let _ = DtnMessage::from_wire_bytes(b"");
        let _ = DtnMessage::from_wire_bytes(b"   ");
    }
}
