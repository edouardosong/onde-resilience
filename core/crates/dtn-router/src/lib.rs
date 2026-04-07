//! DTN Router — Store-and-Forward Delay Tolerant Network
//!
//! Handles message buffering, opportunistic forwarding,
//! and delivery when end-to-end paths don't exist.

use std::collections::{HashMap, VecDeque};
use tokio::sync::Mutex;

/// Maximum TTL for DTN messages
const MAX_TTL: u8 = 10;

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

    /// Store a message in this node's buffer
    pub async fn store(&self, node_id: &str, msg: DtnMessage) {
        let mut buffers = self.buffers.lock().await;
        let buf = buffers.entry(node_id.to_string()).or_default();

        if buf.len() >= self.max_buffer {
            // Drop lowest priority message
            self.drop_lowest(buf).await;
            let mut stats = self.stats.lock().await;
            stats.total_dropped += 1;
        }

        buf.push_back(msg);
        let mut stats = self.stats.lock().await;
        stats.total_stored += 1;
    }

    /// Opportunistic forward when two nodes encounter each other
    pub async fn encounter(&self, node_a: &str, node_b: &str) -> (Vec<DtnMessage>, Vec<DtnMessage>) {
        let mut buffers = self.buffers.lock().await;

        let to_a: Vec<DtnMessage> = self.filter_and_forward(node_b, node_a, &mut buffers).await;
        let to_b: Vec<DtnMessage> = self.filter_and_forward(node_a, node_b, &mut buffers).await;

        (to_a, to_b)
    }

    async fn filter_and_forward(
        &self,
        from: &str,
        to: &str,
        buffers: &mut HashMap<String, VecDeque<DtnMessage>>,
    ) -> Vec<DtnMessage> {
        let mut forwarded = Vec::new();
        let mut stats = self.stats.lock().await;

        if let Some(buf) = buffers.get_mut(from) {
            let mut to_remove = Vec::new();
            for (idx, msg) in buf.iter().enumerate() {
                let should_deliver = match &msg.destination {
                    Some(dest) => dest == to,
                    None => true,
                };
                if should_deliver && msg.hop_count < msg.ttl {
                    let mut deliverable = msg.clone();
                    deliverable.hop_count += 1;
                    stats.total_forwarded += 1;
                    stats.total_delivered += 1;
                    forwarded.push(deliverable);
                    to_remove.push(idx);
                } else if msg.hop_count >= msg.ttl {
                    stats.total_expired += 1;
                    to_remove.push(idx);
                }
            }
            for idx in to_remove.into_iter().rev() {
                buf.remove(idx);
            }
        }

        forwarded
    }

    async fn drop_lowest(&self, buf: &mut VecDeque<DtnMessage>) {
        // Find and remove message with lowest priority (highest number)
        let mut lowest_idx = 0;
        let mut lowest_prio = 0;
        for (i, msg) in buf.iter().enumerate() {
            if msg.priority > lowest_prio {
                lowest_prio = msg.priority;
                lowest_idx = i;
            }
        }
        buf.remove(lowest_idx);
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
                    } else {
                        // We'll update TTL after releasing borrow
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
        };

        // Update stats separately
        if expired_count > 0 {
            let mut stats = self.stats.lock().await;
            stats.total_expired += expired_count;
        }

        expired
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_store_and_forward() {
        let router = DtnRouter::new(100);

        let msg = DtnMessage {
            id: "test-1".into(),
            sender: "A".into(),
            destination: Some("B".into()),
            payload: b"hello".to_vec(),
            msg_type: MessageType::Alert,
            ttl: 5,
            hop_count: 0,
            timestamp_ms: 0,
            priority: 5,
        };

        router.store("A", msg).await;
        assert_eq!(router.buffer_size("A").await, 1);

        // Encounter: A meets B, message should forward
        let (to_a, to_b) = router.encounter("A", "B").await;
        assert_eq!(to_a.len(), 0);
        assert_eq!(to_b.len(), 1);
        assert_eq!(to_b[0].id, "test-1");
    }
}