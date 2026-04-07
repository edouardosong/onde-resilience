/// Network layer — Wi-Fi Aware, BLE, LoRa, Ethernet abstraction

/// Transport technology types
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TransportType {
    /// Wi-Fi Aware (NAN) — ~200m range, 50 Mbps
    WifiAware,
    /// Bluetooth Low Energy — ~50m range, 2 Mbps
    BluetoothLe,
    /// LoRa (Meshtastic) — ~5km range, 50 kbps
    Lora,
    /// Ethernet Bridge (desktop only) — 1 Gbps
    EthernetBridge,
}

impl TransportType {
    pub fn range_meters(&self) -> f64 {
        match self {
            TransportType::WifiAware => 200.0,
            TransportType::BluetoothLe => 50.0,
            TransportType::Lora => 5000.0,
            TransportType::EthernetBridge => 1000.0,
        }
    }

    pub fn bandwidth_bps(&self) -> f64 {
        match self {
            TransportType::WifiAware => 50e6,
            TransportType::BluetoothLe => 2e6,
            TransportType::Lora => 50e3,
            TransportType::EthernetBridge => 1e9,
        }
    }
}

/// Interface for mesh transport
#[async_trait::async_trait]
pub trait MeshTransport: Send + Sync {
    /// Initialize the transport
    async fn init(&mut self) -> Result<(), String>;

    /// Send data to a peer
    async fn send(&self, peer_id: &str, data: &[u8]) -> Result<(), String>;

    /// Receive data (event-driven)
    async fn receive(&self) -> Result<(String, Vec<u8>), String>;

    /// Scan for nearby peers
    async fn scan_peers(&self) -> Vec<String>;

    /// Get transport type
    fn transport_type(&self) -> TransportType;

    /// Get local node ID
    fn local_id(&self) -> &str;
}

/// Multi-transport manager for hybrid networking
pub struct MultiTransport {
    active_transports: Vec<Box<dyn MeshTransport>>,
}

impl MultiTransport {
    pub fn new() -> Self {
        Self {
            active_transports: Vec::new(),
        }
    }

    pub fn add_transport(&mut self, transport: Box<dyn MeshTransport>) {
        self.active_transports.push(transport);
    }

    /// Send via best available transport
    pub async fn send_best(&self, peer_id: &str, data: &[u8]) -> Result<(), String> {
        for transport in &self.active_transports {
            if transport.send(peer_id, data).await.is_ok() {
                return Ok(());
            }
        }
        Err("No available transport".to_string())
    }

    pub fn transports(&self) -> &[Box<dyn MeshTransport>] {
        &self.active_transports
    }
}

/*
 * YGGDRASIL IPv6 Mesh Addressing
 * Cryptographic tree-based address assignment
 */

use sha2::{Digest, Sha256};

/// Yggdrasil mesh address generator
pub struct YggdrasilAddress {
    node_id: String,
}

impl YggdrasilAddress {
    pub fn new(node_id: &str) -> Self {
        Self {
            node_id: node_id.to_string(),
        }
    }

    /// Generate IPv6 ULA address: 200:xxxx:...
    pub fn generate_ipv6(&self) -> String {
        let hash = Sha256::digest(format!("ygg:{self.node_id}"));
        let hex = format!("{hash:x}");

        let mut addr = "200".to_string();
        for i in (0..32).step_by(4) {
            addr.push_str(&format!(":{}", &hex[i..i + 4]));
        }
        addr
    }

    /// Calculate tree distance (shorter prefix = closer)
    pub fn tree_distance(addr_a: &str, addr_b: &str) -> u32 {
        let mut common = 0;
        for (ca, cb) in addr_a.chars().zip(addr_b.chars()) {
            if ca == cb {
                common += 1;
            } else {
                break;
            }
        }
        common as u32
    }

    pub fn node_id(&self) -> &str {
        &self.node_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_yggdrasil_address() {
        let addr = YggdrasilAddress::new("test-node");
        let ipv6 = addr.generate_ipv6();
        assert!(ipv6.starts_with("200:"));
        assert!(ipv6.len() > 10);
    }

    #[test]
    fn test_transport_ranges() {
        assert_eq!(TransportType::WifiAware.range_meters(), 200.0);
        assert_eq!(TransportType::Lora.range_meters(), 5000.0);
    }
}