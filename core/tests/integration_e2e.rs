//! Integration End-to-End Tests for ONDE
//!
//! Tests complete workflows across all subsystems:
//! - Alert → Gossip → Reception
//! - ZK Transaction async flow
//! - Voice Memo → STT → Transcription
//! - AI Query → Oracle Response
//! - DTN Store-and-Forward

use onde_core::crypto::{Identity, TxPool};
use onde_core::protocol::{MeshEvent, OndeMessageType, GossipProtocol};
use onde_core::node::{Node, NodeConfig, NodeType};
use onde_core::ai::AiEngine;
use onde_core::storage::{ZimReader, MBTilesRenderer, IpfsSeeder};
use dtn_router::{DtnRouter, DtnMessage, MessageType};

/*
 * Scenario 1: Alert → Gossip → Reception
 *
 * Node A publishes an alert, Node B receives it via gossip,
 * verifies signature, and stores it.
 */
#[tokio::test]
async fn test_alert_gossip_reception() {
    // Create two nodes
    let mut node_a = Node::new(NodeConfig {
        node_type: NodeType::Mobile,
        display_name: "NodeA".to_string(),
        available_ram_mb: 4096,
        storage_gb: 64,
        ..Default::default()
    });

    let mut node_b = Node::new(NodeConfig {
        node_type: NodeType::Mobile,
        display_name: "NodeB".to_string(),
        available_ram_mb: 4096,
        storage_gb: 64,
        ..Default::default()
    });

    // Node A publishes alert
    let alert_content = "Urgence: inondation secteur 3";
    let event = node_a.publish_alert(alert_content.to_string()).await;
    assert!(event.is_ok(), "Alert publish should succeed");
    let event = event.unwrap();

    // Verify event properties
    assert_eq!(event.content, alert_content);
    assert!(matches!(event.kind, OndeMessageType::Alert));
    assert!(!event.id.is_empty());
    // PoW nonce can be 0 if hash("id:0") already has required leading zeros

    // Node B receives event via gossip
    node_b.gossip.add_event(event.clone());

    // Verify gossip state
    assert_eq!(node_b.gossip.known_count(), 1);

    // Verify event can be retrieved from gossip
    let received = node_b.gossip.get_pending_broadcasts();
    assert_eq!(received.len(), 1);
    assert_eq!(received[0].content, alert_content);
}

/*
 * Scenario 2: ZK Transaction Async Flow
 *
 * Node A sends a ZK transaction to Node B,
 * transaction is queued in pool, then committed.
 */
#[tokio::test]
async fn test_zk_transaction_flow() {
    let mut node_a = Node::new(NodeConfig {
        node_type: NodeType::Mobile,
        display_name: "Sender".to_string(),
        available_ram_mb: 4096,
        storage_gb: 64,
        ..Default::default()
    });

    let receiver_pubkey = "deadbeef0123456789abcdef0123456789abcdef0123456789abcdef01234567";

    // Submit transaction
    let tx_result = node_a.send_transaction(receiver_pubkey, 500).await;
    assert!(tx_result.is_ok(), "Transaction submit should succeed");
    let tx = tx_result.unwrap();

    // Verify transaction properties
    assert_eq!(tx.sender, node_a.identity.pubkey_hex());
    assert_eq!(tx.receiver, receiver_pubkey);
    assert_eq!(tx.amount_micro, 500);
    assert!(!tx.zk_proof.commitment.is_empty(), "ZK proof should be generated");

    // Check pool state
    assert_eq!(node_a.tx_pool.pending_count(), 1);
    assert_eq!(node_a.tx_pool.committed_count(), 0);

    // Commit transactions (simulate internet connection)
    let committed = node_a.commit_transactions(10).await;
    assert_eq!(committed.len(), 1);

    // Verify pool state after commit
    assert_eq!(node_a.tx_pool.pending_count(), 0);
    assert_eq!(node_a.tx_pool.committed_count(), 1);
}

/*
 * Scenario 3: Voice Memo → STT → Transcription
 *
 * Node A creates a voice memo event,
 * Node B receives and transcribes it.
 */
#[tokio::test]
async fn test_voice_memo_transcription() {
    use whisper_stt::{WhisperEngine, WhisperConfig};

    // Create voice memo event (simulated)
    let mut voice_event = MeshEvent::new(
        "voice_sender_pubkey",
        OndeMessageType::VoiceMemo,
        "base64_encoded_opus_data_placeholder".to_string(),
        vec!["duration:5s".to_string()],
    );
    voice_event.pow_difficulty = 2;
    assert!(voice_event.compute_pow(1_000_000), "PoW should succeed");

    // Node B receives voice memo
    let mut node_b = Node::new(NodeConfig {
        node_type: NodeType::Mobile,
        display_name: "Receiver".to_string(),
        available_ram_mb: 4096,
        storage_gb: 64,
        ..Default::default()
    });
    node_b.gossip.add_event(voice_event.clone());

    // Verify voice memo stored
    assert_eq!(node_b.gossip.known_count(), 1);

    // Transcribe using mock STT engine
    let mut stt_engine = WhisperEngine::new(WhisperConfig::default()).unwrap();
    stt_engine.load_model().await.unwrap();

    // Simulate audio data (1 second silence at 16kHz)
    let silence = vec![0i16; 16000];
    let transcription = stt_engine.transcribe(&silence, 16000).await.unwrap();

    // Verify transcription
    assert!(!transcription.text.is_empty());
    assert!(transcription.confidence > 0.0);
    assert_eq!(transcription.language, "fr");
}

/*
 * Scenario 4: AI Query → Oracle Response
 *
 * Node A (mobile) queries AI engine,
 * gets response from local model or oracle.
 */
#[tokio::test]
async fn test_ai_query_response() {
    // Create mobile node with AI engine
    let mut node = Node::new(NodeConfig {
        node_type: NodeType::Mobile,
        display_name: "MobileUser".to_string(),
        available_ram_mb: 2048,
        storage_gb: 32,
        ..Default::default()
    });

    // Query AI engine directly
    let response = node.ai_engine.lock().await.infer(
        "Comment faire la RCP (Reanimation Cardio-Pulmonaire) ?",
        256,
    ).await;

    // Verify response
    assert!(!response.text.is_empty(), "AI response should not be empty");
    assert!(response.tokens_generated > 0, "Should have generated tokens");
    assert!(response.latency_ms > 0, "Should have latency metric");

    // Verify response contains relevant first aid info
    let text_lower = response.text.to_lowercase();
    assert!(
        text_lower.contains("compression") || text_lower.contains("cardio") || text_lower.contains("reanimation"),
        "Response should contain first aid related content"
    );
}

/*
 * Scenario 5: DTN Store-and-Forward
 *
 * Node A stores message for offline Node D,
 * when Node D comes online, message is delivered.
 */
#[tokio::test]
async fn test_dtn_store_and_forward() {
    // Create DTN router
    let router = DtnRouter::new(100);

    // Node A creates message for Node D (offline)
    let msg = DtnMessage {
        id: "msg-urgent-1".to_string(),
        sender: "node_a".to_string(),
        destination: Some("node_d".to_string()),
        payload: b"Message urgent pour Node D".to_vec(),
        msg_type: MessageType::Alert,
        ttl: 10,
        hop_count: 0,
        timestamp_ms: 0,
        priority: 1,
    };

    // Store message in DTN buffer (Node D is offline)
    router.store("node_a", msg).await;

    // Verify message is buffered
    assert_eq!(router.buffer_size("node_a").await, 1);

    // Simulate encounter: Node D comes online
    // Node A delivers message to Node D
    let (to_a, to_d) = router.encounter("node_a", "node_d").await;

    // Verify delivery
    assert_eq!(to_a.len(), 0);
    assert_eq!(to_d.len(), 1);
    assert_eq!(to_d[0].id, "msg-urgent-1");

    // Verify buffer is now empty
    assert_eq!(router.buffer_size("node_a").await, 0);

    // Verify stats
    let stats = router.stats().await;
    assert_eq!(stats.total_delivered, 1);
    assert_eq!(stats.total_forwarded, 1);
}

/*
 * Scenario 6: Full Node Lifecycle
 *
 * Start node → publish alert → query AI → check status → stop
 */
#[tokio::test]
async fn test_full_node_lifecycle() {
    let mut node = Node::new(NodeConfig {
        node_type: NodeType::DesktopBridge,
        display_name: "DesktopOracle".to_string(),
        available_ram_mb: 16384,
        storage_gb: 512,
        ..Default::default()
    });

    // Start node
    assert!(node.start().await.is_ok());
    assert!(node.is_running());

    // Publish alert
    let alert = node.publish_alert("Test alert from desktop".to_string()).await;
    assert!(alert.is_ok());

    // Query AI
    let response = node.ai_engine.lock().await.infer(
        "Quelles sont les techniques de survie en foret ?",
        128,
    ).await;
    assert!(!response.text.is_empty());

    // Check status
    let status = node.status().await;
    assert!(status.is_running);
    assert_eq!(status.node_type, NodeType::DesktopBridge);
    assert_eq!(status.gossip_known_events, 1); // alert published
    assert!(!status.pubkey.is_empty());
    assert!(!status.mesh_address.is_empty());

    // Stop node
    node.stop().await;
    assert!(!node.is_running());
}

/*
 * Scenario 7: Multi-Node Gossip Network
 *
 * 5 nodes in mesh network, one publishes alert,
 * all others receive it via gossip propagation.
 */
#[tokio::test]
async fn test_multi_node_gossip() {
    let mut nodes: Vec<Node> = (0..5)
        .map(|i| {
            Node::new(NodeConfig {
                node_type: NodeType::Mobile,
                display_name: format!("Node-{i}"),
                available_ram_mb: 4096,
                storage_gb: 64,
                ..Default::default()
            })
        })
        .collect();

    // Node 0 publishes alert
    let alert = nodes[0].publish_alert("Alerte reseau: tremblement de terre".to_string()).await.unwrap();

    // Propagate through gossip network (simulate flooding)
    for i in 1..5 {
        nodes[i].gossip.add_event(alert.clone());
    }

    // Verify all nodes received the alert
    for i in 1..5 {
        assert_eq!(
            nodes[i].gossip.known_count(),
            1,
            "Node {} should have received the alert",
            i
        );
        let pending = nodes[i].gossip.get_pending_broadcasts();
        assert_eq!(pending[0].content, "Alerte reseau: tremblement de terre");
    }
}

/*
 * Scenario 8: Storage Subsystem Integration
 *
 * Test ZIM search, map tiles, and IPFS seeding together.
 */
#[tokio::test]
async fn test_storage_integration() {
    // ZIM Reader
    let mut zim = ZimReader::new();
    zim.load_archive("/nonexistent/demo.zim").unwrap();
    let results = zim.search("secours");
    assert!(!results.is_empty() || zim.total_articles() > 0);

    // MBTiles Renderer
    let mut maps = MBTilesRenderer::new();
    maps.load("/nonexistent/maps.mbtiles").unwrap();

    // Get tile for Paris at zoom 5 (demo cache has tiles 0..4)
    let tile = maps.get_tile(5, 2, 2);
    assert!(tile.is_some(), "Should have demo tile");

    // Geohash for Paris
    let geohash = MBTilesRenderer::position_to_geohash(48.8566, 2.3522, 7);
    assert_eq!(geohash.len(), 7);

    // IPFS Seeder
    let seeder = IpfsSeeder::new("/tmp/onde-ipfs", 100);
    let seeds = seeder.list_seeds();
    assert!(seeds.len() >= 5, "Should have demo seeds");

    // Verify specific seeds exist
    assert!(seeder.get_seed("QmWikipedia").is_some());
    assert!(seeder.get_seed("QmOndeAPK").is_some());
    assert!(seeder.get_seed("QmQwen08B").is_some());
}

/*
 * Scenario 9: PoW Antispam Stress Test
 *
 * Verify PoW effectively rate-limits message creation.
 */
#[tokio::test]
async fn test_pow_antispam() {
    let identity = Identity::generate();
    let pubkey = identity.pubkey_hex();

    // Create 10 events with PoW difficulty 2
    let mut events = Vec::new();
    for i in 0..10 {
        let mut event = MeshEvent::new(
            &pubkey,
            OndeMessageType::Alert,
            format!("Test message {i}"),
            vec![],
        );
        event.pow_difficulty = 2;
        let success = event.compute_pow(1_000_000);
        assert!(success, "PoW should succeed for difficulty 2");
        events.push(event);
    }

    // Verify all events have valid PoW
    for event in &events {
        // Verify PoW hash has required leading zeros
        let data = format!("{}:{}", event.id, event.pow_nonce);
        use sha2::{Digest, Sha256};
        let hash = Sha256::digest(data.as_bytes());
        let hex = format!("{hash:x}");
        assert!(
            hex.starts_with("00"),
            "PoW hash should have 2 leading zeros, got: {}",
            &hex[..8]
        );
    }
}

/*
 * Scenario 10: Crypto Sign/Verify Chain
 *
 * Message signed by Node A, verified by Nodes B, C, D.
 */
#[tokio::test]
async fn test_crypto_sign_verify_chain() {
    let node_a = Identity::generate();
    let node_b = Identity::generate();
    let node_c = Identity::generate();

    // Node A signs message
    let message = b"Message important a verifier";
    let signature = node_a.sign(message);

    // Node B verifies with Node A's public key
    let pubkey_a = node_a.verifying_key_bytes();
    assert!(
        Identity::verify_from_pubkey(&pubkey_a, message, &signature),
        "Node B should verify Node A's signature"
    );

    // Node C also verifies
    assert!(
        Identity::verify_from_pubkey(&pubkey_a, message, &signature),
        "Node C should verify Node A's signature"
    );

    // Tampered message should fail
    let tampered = b"Message modifie par attaquant";
    assert!(
        !Identity::verify_from_pubkey(&pubkey_a, tampered, &signature),
        "Tampered message should fail verification"
    );
}

/*
 * Scenario 11: DTN TTL Expiration
 *
 * Messages with low TTL expire and are cleaned up.
 */
#[tokio::test]
async fn test_dtn_ttl_expiration() {
    let router = DtnRouter::new(100);

    // Create message with TTL=2
    let msg = DtnMessage {
        id: "ttl-test".to_string(),
        sender: "node_x".to_string(),
        destination: Some("node_y".to_string()),
        payload: b"TTL test".to_vec(),
        msg_type: MessageType::Alert,
        ttl: 2,
        hop_count: 0,
        timestamp_ms: 0,
        priority: 5,
    };

    router.store("node_x", msg).await;
    assert_eq!(router.buffer_size("node_x").await, 1);

    // First tick: TTL becomes 1
    let expired = router.tick("node_x").await;
    assert_eq!(expired.len(), 0);
    assert_eq!(router.buffer_size("node_x").await, 1);

    // Second tick: TTL becomes 0, message expires
    let expired = router.tick("node_x").await;
    assert_eq!(expired.len(), 1);
    assert_eq!(router.buffer_size("node_x").await, 0);

    let stats = router.stats().await;
    assert_eq!(stats.total_expired, 1);
}

/*
 * Scenario 12: DTN Buffer Overflow
 *
 * When buffer is full, lowest priority messages are dropped.
 */
#[tokio::test]
async fn test_dtn_buffer_overflow() {
    let router = DtnRouter::new(3); // Small buffer for testing

    // Fill buffer with 3 messages
    for i in 0..3 {
        let msg = DtnMessage {
            id: format!("msg-{i}"),
            sender: "node_a".to_string(),
            destination: Some("node_b".to_string()),
            payload: format!("Payload {i}").into_bytes(),
            msg_type: MessageType::Alert,
            ttl: 10,
            hop_count: 0,
            timestamp_ms: 0,
            priority: i as u8, // Increasing priority number = lower priority
        };
        router.store("node_a", msg).await;
    }

    assert_eq!(router.buffer_size("node_a").await, 3);

    // Add 4th message - should drop lowest priority (msg-2, priority=2)
    let msg4 = DtnMessage {
        id: "msg-3".to_string(),
        sender: "node_a".to_string(),
        destination: Some("node_b".to_string()),
        payload: b"High priority".to_vec(),
        msg_type: MessageType::Alert,
        ttl: 10,
        hop_count: 0,
        timestamp_ms: 0,
        priority: 0, // Highest priority
    };
    router.store("node_a", msg4).await;

    // Buffer should still be at max (3), but one was dropped
    assert_eq!(router.buffer_size("node_a").await, 3);

    let stats = router.stats().await;
    assert_eq!(stats.total_dropped, 1);
}