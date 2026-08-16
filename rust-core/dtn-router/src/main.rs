//! ONDE DTN Router - Delay Tolerant Network Implementation
//! 
//! This module implements the core DTN routing protocol for the ONDE resilience network.
//! It handles message bundling, custody transfer, and epidemic routing with optimizations.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use chrono::{DateTime, Utc};
use log::{info, warn, debug, error};
use thiserror::Error;

/// Maximum bundle size in bytes
const MAX_BUNDLE_SIZE: usize = 1024 * 1024; // 1MB
/// Maximum queue length per destination
const MAX_QUEUE_LENGTH: usize = 1000;
/// TTL for bundles in seconds
const DEFAULT_TTL: Duration = Duration::from_secs(3600); // 1 hour

#[derive(Error, Debug)]
pub enum DtnError {
    #[error("Bundle too large: {0} bytes")]
    BundleTooLarge(usize),
    #[error("Queue full for destination {0}")]
    QueueFull(String),
    #[error("Invalid destination: {0}")]
    InvalidDestination(String),
    #[error("Serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleId(pub String);

impl BundleId {
    pub fn new() -> Self {
        BundleId(Uuid::new_v4().to_string())
    }
}

impl Default for BundleId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Priority {
    Low,
    Normal,
    High,
    Critical,
}

impl Default for Priority {
    fn default() -> Self {
        Priority::Normal
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bundle {
    pub id: BundleId,
    pub source: String,
    pub destination: String,
    pub payload: Vec<u8>,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub priority: Priority,
    pub hop_count: u32,
    pub custody_chain: Vec<String>,
}

impl Bundle {
    pub fn new(source: String, destination: String, payload: Vec<u8>) -> Result<Self, DtnError> {
        if payload.len() > MAX_BUNDLE_SIZE {
            return Err(DtnError::BundleTooLarge(payload.len()));
        }

        let now = Utc::now();
        Ok(Bundle {
            id: BundleId::new(),
            source,
            destination,
            payload,
            created_at: now,
            expires_at: now + DEFAULT_TTL,
            priority: Priority::Normal,
            hop_count: 0,
            custody_chain: Vec::new(),
        })
    }

    pub fn is_expired(&self) -> bool {
        Utc::now() > self.expires_at
    }

    pub fn add_custody(&mut self, node_id: String) {
        self.custody_chain.push(node_id);
        self.hop_count += 1;
    }

    pub fn ttl_remaining(&self) -> Duration {
        let now = Utc::now();
        if now >= self.expires_at {
            Duration::ZERO
        } else {
            (self.expires_at - now).to_std().unwrap_or(Duration::ZERO)
        }
    }
}

#[derive(Debug, Clone)]
pub struct Encounter {
    pub peer_id: String,
    pub timestamp: DateTime<Utc>,
    pub signal_strength: f32,
    pub duration: Duration,
}

pub struct DtnRouter {
    node_id: String,
    queues: HashMap<String, VecDeque<Bundle>>,
    encountered_peers: HashMap<String, Encounter>,
    delivery_stats: DeliveryStats,
}

#[derive(Debug, Default)]
pub struct DeliveryStats {
    pub bundles_sent: u64,
    pub bundles_received: u64,
    pub bundles_delivered: u64,
    pub bundles_expired: u64,
    pub total_hops: u64,
}

impl DtnRouter {
    pub fn new(node_id: String) -> Self {
        info!("Initializing DTN Router for node: {}", node_id);
        DtnRouter {
            node_id,
            queues: HashMap::new(),
            encountered_peers: HashMap::new(),
            delivery_stats: DeliveryStats::default(),
        }
    }

    pub fn enqueue_bundle(&mut self, bundle: Bundle) -> Result<(), DtnError> {
        if bundle.is_expired() {
            warn!("Attempted to enqueue expired bundle: {}", bundle.id.0);
            self.delivery_stats.bundles_expired += 1;
            return Ok(());
        }

        let dest = bundle.destination.clone();
        let queue = self.queues.entry(dest.clone()).or_insert_with(VecDeque::new);

        if queue.len() >= MAX_QUEUE_LENGTH {
            // Remove oldest low priority bundle if queue is full
            if let Some(pos) = queue.iter().position(|b| matches!(b.priority, Priority::Low)) {
                queue.remove(pos);
                debug!("Removed low priority bundle to make room");
            } else {
                return Err(DtnError::QueueFull(dest));
            }
        }

        queue.push_back(bundle);
        self.delivery_stats.bundles_sent += 1;
        debug!("Bundle enqueued: {}", dest);
        Ok(())
    }

    pub fn receive_bundle(&mut self, mut bundle: Bundle) -> Result<(), DtnError> {
        if bundle.is_expired() {
            warn!("Received expired bundle: {}", bundle.id.0);
            self.delivery_stats.bundles_expired += 1;
            return Ok(());
        }

        bundle.add_custody(self.node_id.clone());
        self.delivery_stats.bundles_received += 1;

        if bundle.destination == self.node_id {
            // Deliver locally
            info!("Bundle delivered locally: {}", bundle.id.0);
            self.delivery_stats.bundles_delivered += 1;
            self.process_local_delivery(bundle)?;
        } else {
            // Forward bundle
            self.enqueue_bundle(bundle)?;
        }

        Ok(())
    }

    fn process_local_delivery(&self, bundle: Bundle) -> Result<(), DtnError> {
        // In a real implementation, this would deliver to the application layer
        debug!("Processing local delivery for bundle: {}", bundle.id.0);
        Ok(())
    }

    pub fn record_encounter(&mut self, peer_id: String, signal_strength: f32, duration: Duration) {
        let encounter = Encounter {
            peer_id: peer_id.clone(),
            timestamp: Utc::now(),
            signal_strength,
            duration,
        };
        
        self.encountered_peers.insert(peer_id.clone(), encounter);
        debug!("Recorded encounter with peer: {}", peer_id);
    }

    pub fn get_bundles_for_peer(&mut self, peer_id: &str) -> Vec<Bundle> {
        let mut bundles_to_send = Vec::new();

        // Epidemic routing: send all bundles we have that the peer doesn't
        let dests_to_process: Vec<String> = self.queues.keys().cloned().collect();
        
        for dest in dests_to_process {
            let should_forward = dest == peer_id || self.should_forward_to_peer(peer_id, &dest);
            
            if let Some(queue) = self.queues.get_mut(&dest) {
                let mut remaining_queue = VecDeque::new();
                
                while let Some(bundle) = queue.pop_front() {
                    if bundle.is_expired() {
                        self.delivery_stats.bundles_expired += 1;
                        continue;
                    }

                    if should_forward {
                        bundles_to_send.push(bundle);
                    } else {
                        remaining_queue.push_back(bundle);
                    }
                }

                *queue = remaining_queue;
            }
        }

        debug!("Prepared {} bundles for peer {}", bundles_to_send.len(), peer_id);
        bundles_to_send
    }

    fn should_forward_to_peer(&self, peer_id: &str, _destination: &str) -> bool {
        // Simple heuristic: forward to recently encountered peers
        // In production, this would use more sophisticated routing metrics
        self.encountered_peers.contains_key(peer_id)
    }

    pub fn cleanup_expired(&mut self) -> usize {
        let mut expired_count = 0;

        for (_dest, queue) in &mut self.queues.iter_mut() {
            let original_len = queue.len();
            *queue = queue.drain(..).filter(|b| !b.is_expired()).collect();
            expired_count += original_len - queue.len();
        }

        if expired_count > 0 {
            self.delivery_stats.bundles_expired += expired_count as u64;
            info!("Cleaned up {} expired bundles", expired_count);
        }

        expired_count
    }

    pub fn get_stats(&self) -> &DeliveryStats {
        &self.delivery_stats
    }

    pub fn get_queue_depth(&self, destination: &str) -> usize {
        self.queues.get(destination).map(|q| q.len()).unwrap_or(0)
    }

    pub fn total_queued_bundles(&self) -> usize {
        self.queues.values().map(|q| q.len()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bundle_creation() {
        let bundle = Bundle::new(
            "node1".to_string(),
            "node2".to_string(),
            vec![1, 2, 3],
        ).unwrap();

        assert_eq!(bundle.source, "node1");
        assert_eq!(bundle.destination, "node2");
        assert_eq!(bundle.payload, vec![1, 2, 3]);
        assert_eq!(bundle.hop_count, 0);
    }

    #[test]
    fn test_bundle_too_large() {
        let large_payload = vec![0u8; MAX_BUNDLE_SIZE + 1];
        let result = Bundle::new(
            "node1".to_string(),
            "node2".to_string(),
            large_payload,
        );

        assert!(matches!(result, Err(DtnError::BundleTooLarge(_))));
    }

    #[test]
    fn test_router_enqueue_dequeue() {
        let mut router = DtnRouter::new("router1".to_string());
        
        let bundle = Bundle::new(
            "node1".to_string(),
            "node2".to_string(),
            vec![1, 2, 3],
        ).unwrap();

        router.enqueue_bundle(bundle).unwrap();
        assert_eq!(router.get_queue_depth("node2"), 1);
        assert_eq!(router.total_queued_bundles(), 1);
    }

    #[test]
    fn test_bundle_custody_chain() {
        let mut bundle = Bundle::new(
            "node1".to_string(),
            "node3".to_string(),
            vec![1, 2, 3],
        ).unwrap();

        bundle.add_custody("node2".to_string());
        assert_eq!(bundle.hop_count, 1);
        assert_eq!(bundle.custody_chain.len(), 1);
        assert_eq!(bundle.custody_chain[0], "node2");

        bundle.add_custody("node3".to_string());
        assert_eq!(bundle.hop_count, 2);
        assert_eq!(bundle.custody_chain.len(), 2);
    }

    #[test]
    fn test_priority_queue_management() {
        let mut router = DtnRouter::new("router1".to_string());

        // Fill queue with normal priority bundles
        for i in 0..MAX_QUEUE_LENGTH {
            let bundle = Bundle::new(
                "node1".to_string(),
                "node2".to_string(),
                vec![i as u8],
            ).unwrap();
            router.enqueue_bundle(bundle).unwrap();
        }

        // Try to add another normal priority bundle (should fail)
        let bundle = Bundle::new(
            "node1".to_string(),
            "node2".to_string(),
            vec![99],
        ).unwrap();
        assert!(matches!(router.enqueue_bundle(bundle), Err(DtnError::QueueFull(_))));

        // Add low priority bundle
        let mut low_bundle = Bundle::new(
            "node1".to_string(),
            "node2".to_string(),
            vec![100],
        ).unwrap();
        low_bundle.priority = Priority::Low;
        router.enqueue_bundle(low_bundle).unwrap();

        // Now try to add high priority bundle (should succeed by evicting low priority)
        let mut high_bundle = Bundle::new(
            "node1".to_string(),
            "node2".to_string(),
            vec![200],
        ).unwrap();
        high_bundle.priority = Priority::High;
        router.enqueue_bundle(high_bundle).unwrap();

        assert_eq!(router.get_queue_depth("node2"), MAX_QUEUE_LENGTH);
    }
}

fn main() {
    env_logger::init();
    
    info!("Starting ONDE DTN Router v1.0.0");
    
    let mut router = DtnRouter::new("demo_node".to_string());
    
    // Create and enqueue some test bundles
    for i in 0..5 {
        let bundle = Bundle::new(
            format!("sender_{}", i),
            "receiver".to_string(),
            format!("Message {}", i).into_bytes(),
        ).unwrap();
        
        if let Err(e) = router.enqueue_bundle(bundle) {
            error!("Failed to enqueue bundle: {}", e);
        }
    }

    info!("Total queued bundles: {}", router.total_queued_bundles());
    info!("Router stats: {:?}", router.get_stats());
    
    println!("ONDE DTN Router initialized successfully!");
    println!("Queued bundles: {}", router.total_queued_bundles());
    println!("Stats: {:?}", router.get_stats());
}
