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

pub mod tcp;

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

impl Default for MultiTransport {
    fn default() -> Self {
        Self::new()
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

    /// Generate a deterministic ULA (Unique Local Address) in fd00::/8 derived
    /// from the node identity:
    /// - byte 0: 0xfd (ULA prefix, fd00::/8)
    /// - byte 1: 7-bit scope derived from the identity hash
    /// - bytes 2..9: first 7 bytes of SHA-256("ygg:{node_id}")
    /// - remaining bytes: 0
    ///
    /// Stable for the same identity, distinct across identities.
    pub fn generate_ipv6(&self) -> String {
        let hash = Sha256::digest(format!("ygg:{}", self.node_id));
        let mut octets = [0u8; 16];
        octets[0] = 0xfd; // ULA fd00::/8
        octets[1] = hash[7] & 0x7f; // 7-bit scope derived from the identity
        octets[2..9].copy_from_slice(&hash[0..7]); // 7 identity-derived bytes
        std::net::Ipv6Addr::from(octets).to_string()
    }

    /// Tree distance between two IPv6 addresses: the number of common
    /// most-significant prefix bits over the full 128 bits.
    ///
    /// Larger = closer in the tree. Identical addresses → 128, addresses with
    /// no common prefix bit → 0. Returns `Err` if either string does not parse
    /// as an `Ipv6Addr`.
    pub fn tree_distance(addr_a: &str, addr_b: &str) -> Result<u32, String> {
        let a = addr_a
            .parse::<std::net::Ipv6Addr>()
            .map_err(|e| format!("invalid IPv6 address '{addr_a}': {e}"))?;
        let b = addr_b
            .parse::<std::net::Ipv6Addr>()
            .map_err(|e| format!("invalid IPv6 address '{addr_b}': {e}"))?;
        Ok(common_prefix_bits(&a.octets(), &b.octets()))
    }

    pub fn node_id(&self) -> &str {
        &self.node_id
    }
}

/// Number of common most-significant prefix bits between two 16-byte addresses.
/// Identical addresses return 128.
fn common_prefix_bits(a: &[u8; 16], b: &[u8; 16]) -> u32 {
    for i in 0..16 {
        if a[i] != b[i] {
            // All previous octets are identical (8 bits each); count the common
            // MSB bits of the first differing octet and stop.
            return (i as u32) * 8 + common_msb_bits(a[i], b[i]);
        }
    }
    128
}

/// Number of common most-significant bits of a single byte pair (0..=8).
fn common_msb_bits(x: u8, y: u8) -> u32 {
    (x ^ y).leading_zeros()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_yggdrasil_address_valid_ula() {
        let addr = YggdrasilAddress::new("test-node");
        let ipv6 = addr.generate_ipv6();

        // Must parse as a real IPv6 address
        let parsed: std::net::Ipv6Addr = ipv6
            .parse()
            .expect("generated address must be a valid IPv6 address");
        // ULA fd00::/8: starts with "fd", first octet is 0xfd
        assert!(
            ipv6.starts_with("fd"),
            "ULA address must start with fd, got: {ipv6}"
        );
        assert_eq!(parsed.octets()[0], 0xfd);
        assert_eq!(parsed.octets().len(), 16);
    }

    #[test]
    fn test_yggdrasil_address_stable_per_identity() {
        let a = YggdrasilAddress::new("node-1");
        let b = YggdrasilAddress::new("node-1");
        assert_eq!(
            a.generate_ipv6(),
            b.generate_ipv6(),
            "same identity must give the same address"
        );
    }

    #[test]
    fn test_yggdrasil_address_distinct_per_identity() {
        let a = YggdrasilAddress::new("node-1");
        let b = YggdrasilAddress::new("node-2");
        let (addr_a, addr_b) = (a.generate_ipv6(), b.generate_ipv6());
        assert_ne!(
            addr_a, addr_b,
            "different identities must give different addresses"
        );
        // Both remain valid ULAs
        assert!(addr_a.starts_with("fd"));
        assert!(addr_b.starts_with("fd"));
    }

    #[test]
    fn test_tree_distance_identical() {
        let addr = "fd00::1";
        assert_eq!(YggdrasilAddress::tree_distance(addr, addr).unwrap(), 128);
    }

    #[test]
    fn test_tree_distance_same_binary_different_text() {
        // Same binary address, different textual representations
        assert_eq!(
            YggdrasilAddress::tree_distance("fd00::1", "fd00:0:0:0:0:0:0:1").unwrap(),
            128
        );
    }

    #[test]
    fn test_tree_distance_differ_in_last_bit() {
        // Identical except the very last bit (last octet 0x00 vs 0x01)
        assert_eq!(
            YggdrasilAddress::tree_distance("fd00::", "fd00::1").unwrap(),
            127
        );
    }

    #[test]
    fn test_tree_distance_one_common_bit() {
        // 0x20 (0010_0000) vs 0x40 (0100_0000): exactly one common MSB bit.
        // (The audit example "2001:db8::1" vs "fd00::1" actually shares 0 bits:
        // 0x20 vs 0xfd differ on the very first bit — use a correct pair here.)
        assert_eq!(
            YggdrasilAddress::tree_distance("2000::", "4000::").unwrap(),
            1
        );
    }

    #[test]
    fn test_tree_distance_no_common_bit() {
        // 0x00 (0000_0000) vs 0x80 (1000_0000): no common MSB bit
        assert_eq!(YggdrasilAddress::tree_distance("::", "8000::").unwrap(), 0);
    }

    #[test]
    fn test_tree_distance_invalid() {
        assert!(YggdrasilAddress::tree_distance("not-an-address", "fd00::1").is_err());
        assert!(YggdrasilAddress::tree_distance("fd00::1", "not-an-address").is_err());
    }

    #[test]
    fn test_transport_ranges() {
        assert_eq!(TransportType::WifiAware.range_meters(), 200.0);
        assert_eq!(TransportType::Lora.range_meters(), 5000.0);
    }
}
