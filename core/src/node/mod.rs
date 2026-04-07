/// Node Management — Core ONDE node with all subsystems

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::crypto::{Identity, ZkTransaction, TxPool};
use crate::network::YggdrasilAddress;
use crate::protocol::{MeshEvent, OndeMessageType, GossipProtocol};
use crate::ai::AiEngine;
use crate::storage::{ZimReader, MBTilesRenderer, IpfsSeeder};

/// Node type
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NodeType {
    /// Mobile device (phone/tablet)
    Mobile,
    /// Desktop/Laptop bridge (ethernet + AI oracle)
    DesktopBridge,
}

/// Node configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeConfig {
    pub node_type: NodeType,
    pub display_name: String,
    pub available_ram_mb: u64,
    pub storage_gb: u64,
    pub ai_model_preference: Option<String>,
    pub max_peer_connections: u32,
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            node_type: NodeType::Mobile,
            display_name: "Unknown".to_string(),
            available_ram_mb: 4096,
            storage_gb: 64,
            ai_model_preference: None,
            max_peer_connections: 20,
        }
    }
}

/// The main ONDE node
pub struct Node {
    pub config: NodeConfig,
    pub identity: Identity,
    pub mesh_address: YggdrasilAddress,
    pub gossip: GossipProtocol,
    pub tx_pool: TxPool,
    pub ai_engine: Mutex<AiEngine>,
    pub zim_reader: ZimReader,
    pub map_renderer: MBTilesRenderer,
    pub ipfs_seeder: IpfsSeeder,
    is_running: bool,
}

impl Node {
    pub fn new(config: NodeConfig) -> Self {
        let identity = match config.node_type {
            NodeType::Mobile => Identity::generate(),
            NodeType::DesktopBridge => Identity::generate(),
        };

        let pubkey = identity.pubkey_hex();
        let mesh_address = YggdrasilAddress::new(&pubkey);

        let ai_engine = AiEngine::new(config.available_ram_mb);
        let ipfs_seeder = IpfsSeeder::new("/tmp/onde-ipfs", config.storage_gb);

        Self {
            config,
            identity,
            mesh_address,
            gossip: GossipProtocol::new(),
            tx_pool: TxPool::new(),
            ai_engine: Mutex::new(ai_engine),
            zim_reader: ZimReader::new(),
            map_renderer: MBTilesRenderer::new(),
            ipfs_seeder,
            is_running: false,
        }
    }

    /// Start the node
    pub async fn start(&mut self) -> Result<(), String> {
        tracing::info!(
            "Starting ONDE node [{}] type={:?} pubkey={}",
            self.config.display_name,
            self.config.node_type,
            self.identity.pubkey_hex()
        );

        self.is_running = true;
        Ok(())
    }

    /// Stop the node
    pub async fn stop(&mut self) {
        tracing::info!("Stopping ONDE node...");
        self.is_running = false;
    }

    /// Publish an alert message
    pub async fn publish_alert(&mut self, content: String) -> Result<MeshEvent, String> {
        if content.len() > 280 {
            return Err("Alert must be <= 280 characters".to_string());
        }

        let mut event = MeshEvent::new(
            &self.identity.pubkey_hex(),
            OndeMessageType::Alert,
            content,
            vec![],
        );

        // Compute PoW before publishing
        if !event.compute_pow(100_000) {
            return Err("PoW computation failed".to_string());
        }

        self.gossip.add_event(event.clone());
        Ok(event)
    }

    /// Publish a mutual aid request
    pub async fn publish_mutual_aid(&mut self, content: String) -> Result<MeshEvent, String> {
        let mut event = MeshEvent::new(
            &self.identity.pubkey_hex(),
            OndeMessageType::MutualAid,
            content,
            vec![],
        );

        if !event.compute_pow(100_000) {
            return Err("PoW computation failed".to_string());
        }

        self.gossip.add_event(event.clone());
        Ok(event)
    }

    /// Send a ZK transaction
    pub async fn send_transaction(
        &mut self,
        receiver: &str,
        amount_micro: u64,
    ) -> Result<ZkTransaction, String> {
        let nonce = self.tx_pool.pending_count() as u64;
        let tx = ZkTransaction::new(&self.identity.pubkey_hex(), receiver, amount_micro, nonce);

        self.tx_pool.submit(tx.clone())?;
        Ok(tx)
    }

    /// Commit pending transactions (when internet available)
    pub async fn commit_transactions(&mut self, max_batch: usize) -> Vec<ZkTransaction> {
        self.tx_pool.commit_pending(max_batch)
    }

    /// Check if running
    pub fn is_running(&self) -> bool {
        self.is_running
    }

    /// Get node status summary
    pub async fn status(&self) -> NodeStatus {
        let ai = self.ai_engine.lock().await;
        NodeStatus {
            is_running: self.is_running,
            node_type: self.config.node_type,
            pubkey: self.identity.pubkey_hex(),
            mesh_address: self.mesh_address.generate_ipv6(),
            gossip_known_events: self.gossip.known_count(),
            pending_tx: self.tx_pool.pending_count(),
            committed_tx: self.tx_pool.committed_count(),
            ipfs_seeds: self.ipfs_seeder.list_seeds().len(),
            local_model: ai.get_local_model().map(|m| format!("{m:?}")),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct NodeStatus {
    pub is_running: bool,
    pub node_type: NodeType,
    pub pubkey: String,
    pub mesh_address: String,
    pub gossip_known_events: usize,
    pub pending_tx: usize,
    pub committed_tx: usize,
    pub ipfs_seeds: usize,
    pub local_model: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_node_creation() {
        let config = NodeConfig::default();
        let node = Node::new(config);
        assert!(node.identity.pubkey_hex().len() == 64); // hex 32 bytes
    }

    #[tokio::test]
    async fn test_node_alert_publish() {
        let mut node = Node::new(NodeConfig::default());
        let result = node.publish_alert("Test alert".to_string()).await;
        assert!(result.is_ok());
    }
}