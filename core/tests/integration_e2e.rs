//! Integration End-to-End Tests for ONDE
//!
//! Tests complete workflows across all subsystems:
//! - Alert → Gossip → Reception
//! - ZK Transaction async flow
//! - Voice Memo → STT → Transcription
//! - AI Query → Oracle Response
//! - DTN Store-and-Forward

use onde_core::crypto::Identity;
use onde_core::protocol::{MeshEvent, OndeMessageType};
use onde_core::node::{Node, NodeConfig, NodeType, UpdateHandlingOutcome, EndorsementHandlingOutcome};
use onde_core::reputation::{Endorsement, TRUSTED_THRESHOLD};
use onde_core::update::{UpdateProtocol, Version, DEFAULT_CHUNK_SIZE};
use onde_core::storage::{ZimReader, MBTilesRenderer, IpfsSeeder};
use dtn_router::{DtnRouter, DtnMessage, MessageType};
use base64::Engine as _;

/// Déplacer les événements en attente du gossip de `from` vers `to`
/// (validation adaptative par réputation), puis les faire traiter par le
/// nœud receveur. Retourne le nombre d'événements nouvellement traités.
fn gossip_sync(from: &mut Node, to: &mut Node) -> Result<usize, String> {
    let peer_id = to.identity.pubkey_hex();
    let events = from.gossip.get_pending_for_peer(&peer_id);
    let mut handled = 0;
    for event in events {
        let reputation = to.reputation.clone();
        if to
            .gossip
            .add_event_with_reputation(event.clone(), &reputation)?
        {
            match event.kind {
                // Phase 1.2 : les endossements WoT sont intégrés (et relayés)
                // par leur propre handler ; tout le reste passe par l'update.
                OndeMessageType::Endorsement => {
                    to.handle_incoming_endorsement(&event);
                }
                _ => {
                    to.handle_incoming_update(&event)?;
                }
            }
            handled += 1;
        }
    }
    Ok(handled)
}

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

    // Node A publishes alert (self-trusted → PoW adaptatif = 0)
    let alert_content = "Urgence: inondation secteur 3";
    let event = node_a.publish_alert(alert_content.to_string()).await;
    assert!(event.is_ok(), "Alert publish should succeed");
    let event = event.unwrap();

    // Verify event properties
    assert_eq!(event.content, alert_content);
    assert!(matches!(event.kind, OndeMessageType::Alert));
    assert!(!event.id.is_empty());
    // PoW nonce can be 0 if hash("id:0") already has required leading zeros

    // Node B connaît A comme pair de confiance (Web of Trust) — c'est ce qui
    // lui permet d'accepter un événement à difficulté 0 sans se faire spammer.
    node_b
        .reputation
        .bootstrap(&[node_a.identity.pubkey_hex()]);

    // Node B receives event via gossip (validation adaptative par réputation)
    assert!(
        node_b
            .gossip
            .add_event_with_reputation(event.clone(), &node_b.reputation)
            .is_ok(),
        "Valid signed + PoW alert must be accepted by gossip"
    );

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

    // Create voice memo event (simulated) — signed by a real identity so that
    // gossip validation (signature + PoW) accepts it
    let voice_sender = Identity::generate();
    let mut voice_event = MeshEvent::new_signed(
        &voice_sender,
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
    assert!(
        node_b.gossip.add_event(voice_event.clone()).is_ok(),
        "Signed voice memo must be accepted by gossip"
    );

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
    let node = Node::new(NodeConfig {
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
        delivered_to: vec![],
    };

    // Store message in DTN buffer (Node D is offline)
    assert!(router.store("node_a", msg).await, "Message should be stored");

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

    // Node 0 publishes alert (self-trusted → PoW adaptatif = 0)
    let alert = nodes[0].publish_alert("Alerte reseau: tremblement de terre".to_string()).await.unwrap();

    // Propagation par gossip (simulation de flooding).
    // Chaque nœud connaît Node 0 comme pair de confiance (WoT), sinon un
    // événement à difficulté 0 serait (correctement) rejeté comme spam.
    let publisher_pubkey = nodes[0].identity.pubkey_hex();
    for (i, node) in nodes.iter_mut().enumerate().skip(1) {
        node.reputation.bootstrap(std::slice::from_ref(&publisher_pubkey));
        // Clone de la réputation : emprunt disjoint via index impossible sinon
        let rep = node.reputation.clone();
        assert!(
            node.gossip
                .add_event_with_reputation(alert.clone(), &rep)
                .is_ok(),
            "Node {} should accept the signed alert",
            i
        );
    }

    // Verify all nodes received the alert
    for node in nodes.iter().skip(1) {
        assert_eq!(
            node.gossip.known_count(),
            1,
            "every node should have received the alert"
        );
        let pending = node.gossip.get_pending_broadcasts();
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
    // ZIM Reader — demo mode is explicit (missing files fail loudly)
    let mut zim = ZimReader::new();
    zim.load_demo();
    let results = zim.search("secours");
    assert!(!results.is_empty(), "demo mode must expose searchable articles");
    assert!(zim.total_articles() >= 5);

    // MBTiles Renderer — demo mode is explicit
    let mut maps = MBTilesRenderer::new();
    maps.load_demo();

    // Get tile for Paris at zoom 5 (demo cache has tiles 0..4)
    let tile = maps.get_tile(5, 2, 2);
    assert!(tile.is_some(), "Should have demo tile");

    // Geohash for Paris
    let geohash = MBTilesRenderer::position_to_geohash(48.8566, 2.3522, 7);
    assert_eq!(geohash.len(), 7);

    // IPFS Seeder — demo seeds are registered explicitly
    let mut seeder = IpfsSeeder::new("/tmp/onde-ipfs", 100).expect("seeder should initialize");
    seeder.register_demo_seeds();
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
    let _node_b = Identity::generate();
    let _node_c = Identity::generate();

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
        delivered_to: vec![],
    };

    assert!(router.store("node_x", msg).await, "Message should be stored");
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
            delivered_to: vec![],
        };
        assert!(router.store("node_a", msg).await);
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
        delivered_to: vec![],
    };
    assert!(router.store("node_a", msg4).await, "More urgent message must be stored");

    // Buffer should still be at max (3), but one was dropped
    assert_eq!(router.buffer_size("node_a").await, 3);

    let stats = router.stats().await;
    assert_eq!(stats.total_dropped, 1);
}

/*
 * Scenario 13: Update APK — Distribution sécurisée entre deux nœuds (Phase 1.1)
 *
 * Le nœud A (distribution, détenteur de la clé racine) annonce la version
 * 2.0.0 ; le nœud B (à jour sur 1.0.0) traverse tout le flux gossip :
 * annonce → requête manifeste → manifeste → requêtes chunks → chunks →
 * assemblage → vérification de bout en bout → installation.
 */
#[tokio::test]
async fn test_update_flow_between_two_nodes() -> Result<(), String> {
    // Clé racine de distribution : A détient la seed, les deux épinglent la
    // même clé publique (root pinning).
    let root = Identity::generate();
    let root_pubkey = root.verifying_key_bytes();
    let root_seed = root.signing_key_bytes();

    // APK de démonstration (~40 Kio → 3 chunks de 16 Kio).
    let apk: Vec<u8> = (0..40_000u32).map(|i| (i % 251) as u8).collect();

    let mut node_a = Node::new(NodeConfig {
        node_type: NodeType::DesktopBridge,
        display_name: "Distributeur".to_string(),
        available_ram_mb: 4096,
        storage_gb: 64,
        update_root_pubkey: Some(root_pubkey),
        update_root_seed: Some(root_seed),
        update_version: "2.0.0".to_string(),
        ..Default::default()
    });

    let mut node_b = Node::new(NodeConfig {
        node_type: NodeType::Mobile,
        display_name: "Receveur".to_string(),
        available_ram_mb: 4096,
        storage_gb: 64,
        update_root_pubkey: Some(root_pubkey),
        update_version: "1.0.0".to_string(),
        ..Default::default()
    });

    // Web of Trust réciproque : PoW adaptatif = 0 pour les deux sens.
    let a_pubkey = node_a.identity.pubkey_hex();
    let b_pubkey = node_b.identity.pubkey_hex();
    node_b.reputation.bootstrap(std::slice::from_ref(&a_pubkey));
    node_a.reputation.bootstrap(std::slice::from_ref(&b_pubkey));

    // 1. Le distributeur annonce la version 2.0.0 (signée par la racine).
    let announced = node_a
        .announce_update(Version::new(2, 0, 0), &apk, 1_800_000_000)?;
    assert!(matches!(announced.kind, OndeMessageType::UpdateAnnounce));
    assert!(!announced.sig.is_empty(), "announcement must be signed");

    // 2. Boucle gossip jusqu'à l'installation côté receveur.
    let mut iterations = 0;
    loop {
        gossip_sync(&mut node_a, &mut node_b)?;
        gossip_sync(&mut node_b, &mut node_a)?;
        if node_b.update_protocol.latest_installed().is_some() {
            break;
        }
        iterations += 1;
        assert!(iterations < 100, "update flow did not complete");
    }

    // 3. B a installé la version supérieure.
    assert_eq!(
        node_b.update_protocol.current_version(),
        Version::new(2, 0, 0),
        "receiver must have installed the higher version"
    );
    let installed = node_b
        .update_protocol
        .latest_installed()
        .expect("update must be recorded as installed")
        .clone();
    assert_eq!(installed.version, Version::new(2, 0, 0));

    // 4. L'APK installé est identique byte-à-byte à l'APK annoncé.
    let installed_path = installed.apk_path.clone().expect("desktop install path");
    let installed_bytes = std::fs::read(&installed_path).map_err(|e| e.to_string())?;
    assert_eq!(
        installed_bytes, apk,
        "installed APK must match the announced APK byte-for-byte"
    );
    assert_eq!(
        installed.apk_sha256,
        UpdateProtocol::build_announcement(Version::new(2, 0, 0), &apk, &root, 0).0.apk_sha256,
        "installed hash must match the announced hash"
    );

    // 5. Une version NON supérieure (égale puis downgrade) est rejetée.
    let equal = node_a.announce_update(Version::new(2, 0, 0), &apk, 1_800_000_001)?;
    let reputation = node_b.reputation.clone();
    assert!(
        node_b.gossip.add_event_with_reputation(equal.clone(), &reputation).unwrap()
    );
    match node_b.handle_incoming_update(&equal) {
        Ok(UpdateHandlingOutcome::Rejected(reason)) => {
            assert!(reason.contains("not newer"), "equal version must be rejected: {reason}");
        }
        other => panic!("equal version must be rejected, got {other:?}"),
    }

    let downgrade = node_a.announce_update(Version::new(1, 0, 0), &apk, 1_800_000_002)?;
    let reputation = node_b.reputation.clone();
    assert!(
        node_b.gossip.add_event_with_reputation(downgrade.clone(), &reputation).unwrap()
    );
    match node_b.handle_incoming_update(&downgrade) {
        Ok(UpdateHandlingOutcome::Rejected(reason)) => {
            assert!(reason.contains("not newer"), "downgrade must be rejected: {reason}");
        }
        other => panic!("downgrade must be rejected, got {other:?}"),
    }
    assert_eq!(
        node_b.update_protocol.current_version(),
        Version::new(2, 0, 0),
        "rejected announcements must not change the installed version"
    );

    // Nettoyage de l'APK installé de démonstration.
    let _ = std::fs::remove_file(installed_path);
    Ok(())
}

/*
 * Scenario 14: Update APK — un APK falsifié est rejeté (Phase 1.1)
 *
 * Même flux gossip, mais le chunk 0 servi est falsifié : l'assemblage
 * échoue la vérification de bout en bout, le transfert est purgé et aucune
 * installation n'a lieu.
 */
#[tokio::test]
async fn test_update_rejects_tampered_apk() -> Result<(), String> {
    let root = Identity::generate();
    let root_pubkey = root.verifying_key_bytes();
    let root_seed = root.signing_key_bytes();
    let apk: Vec<u8> = (0..40_000u32).map(|i| (i % 251) as u8).collect();

    let mut node_a = Node::new(NodeConfig {
        display_name: "Distributeur".to_string(),
        update_root_pubkey: Some(root_pubkey),
        update_root_seed: Some(root_seed),
        update_version: "2.0.0".to_string(),
        ..Default::default()
    });
    let mut node_b = Node::new(NodeConfig {
        display_name: "Receveur".to_string(),
        update_root_pubkey: Some(root_pubkey),
        update_version: "1.0.0".to_string(),
        ..Default::default()
    });

    let a_pubkey = node_a.identity.pubkey_hex();
    let b_pubkey = node_b.identity.pubkey_hex();
    node_b.reputation.bootstrap(std::slice::from_ref(&a_pubkey));
    node_a.reputation.bootstrap(std::slice::from_ref(&b_pubkey));

    node_a.announce_update(Version::new(2, 0, 0), &apk, 1_800_000_000)?;

    // Livrer l'annonce → B demande le manifeste ; livrer la requête → A sert
    // le manifeste ; livrer le manifeste → B demande le chunk 0.
    for ev in node_a.gossip.get_pending_for_peer(&b_pubkey) {
        let rep = node_b.reputation.clone();
        if node_b.gossip.add_event_with_reputation(ev.clone(), &rep).unwrap() {
            node_b.handle_incoming_update(&ev)?;
        }
    }
    for ev in node_b.gossip.get_pending_for_peer(&a_pubkey) {
        let rep = node_a.reputation.clone();
        if node_a.gossip.add_event_with_reputation(ev.clone(), &rep).unwrap() {
            node_a.handle_incoming_update(&ev)?;
        }
    }
    for ev in node_a.gossip.get_pending_for_peer(&b_pubkey) {
        let rep = node_b.reputation.clone();
        if node_b.gossip.add_event_with_reputation(ev.clone(), &rep).unwrap() {
            node_b.handle_incoming_update(&ev)?;
        }
    }
    assert!(node_b.update_protocol.has_pending(), "transfer must be initialized");
    assert_eq!(node_b.update_protocol.chunks_received(), 0);

    // Le chunk 0 servi par A est FALSIFIÉ (un octet retourné), mais reste
    // signé par l'identité d'annonceur → il passe la validation gossip.
    let total = UpdateProtocol::chunk_count(apk.len(), DEFAULT_CHUNK_SIZE as usize);
    let mut evil_chunk0 = UpdateProtocol::chunk(&apk, 0, DEFAULT_CHUNK_SIZE as usize).unwrap();
    evil_chunk0[0] ^= 0xFF;
    let evil_event = MeshEvent::new_signed(
        &node_a.identity,
        OndeMessageType::UpdateChunk,
        base64::engine::general_purpose::STANDARD.encode(&evil_chunk0),
        vec![
            "index=0".to_string(),
            format!("total={total}"),
            format!("peer={a_pubkey}"),
        ],
    )
    .with_pow_difficulty(0); // annonceur de confiance → PoW adaptatif 0
    let rep = node_b.reputation.clone();
    assert!(node_b.gossip.add_event_with_reputation(evil_event.clone(), &rep).is_ok());
    assert_eq!(
        node_b.handle_incoming_update(&evil_event)?,
        UpdateHandlingOutcome::ChunkRequested(1),
        "tampered chunk 0 is accepted as a chunk, next chunk requested"
    );

    // Livrer la requête chunk 1 à A → A sert le chunk 1 réel ; livrer à B.
    for ev in node_b.gossip.get_pending_for_peer(&a_pubkey) {
        let rep = node_a.reputation.clone();
        if node_a.gossip.add_event_with_reputation(ev.clone(), &rep).unwrap() {
            node_a.handle_incoming_update(&ev)?;
        }
    }
    for ev in node_a.gossip.get_pending_for_peer(&b_pubkey) {
        let rep = node_b.reputation.clone();
        if node_b.gossip.add_event_with_reputation(ev.clone(), &rep).unwrap() {
            node_b.handle_incoming_update(&ev)?;
        }
    }
    assert_eq!(node_b.update_protocol.chunks_received(), 2);

    // Livrer la requête chunk 2 à A → A sert le chunk 2 réel ; livrer à B →
    // assemblage + vérification : l'APK reconstruit ne correspond pas au hash
    // signé → rejet.
    for ev in node_b.gossip.get_pending_for_peer(&a_pubkey) {
        let rep = node_a.reputation.clone();
        if node_a.gossip.add_event_with_reputation(ev.clone(), &rep).unwrap() {
            node_a.handle_incoming_update(&ev)?;
        }
    }
    let mut saw_reject = false;
    for ev in node_a.gossip.get_pending_for_peer(&b_pubkey) {
        let rep = node_b.reputation.clone();
        if node_b.gossip.add_event_with_reputation(ev.clone(), &rep).unwrap() {
            if let Ok(UpdateHandlingOutcome::Rejected(reason)) = node_b.handle_incoming_update(&ev) {
                assert!(
                    reason.contains("verification"),
                    "tampered APK must fail end-to-end verification: {reason}"
                );
                saw_reject = true;
            }
        }
    }
    assert!(saw_reject, "tampered APK must be rejected at assembly");
    assert!(
        node_b.update_protocol.latest_installed().is_none(),
        "no installation must occur for a tampered APK"
    );
    assert!(!node_b.update_protocol.has_pending(), "poisoned transfer must be purged");
    assert_eq!(
        node_b.update_protocol.current_version(),
        Version::new(1, 0, 0),
        "receiver must stay on its previous version"
    );
    Ok(())
}

/*
 * Scenario 15: WoT — Propagation des endossements entre nœuds (Phase 1.2)
 *
 * A (fondateur de confiance) endosse B. L'endossement signé circule dans le
 * gossip : B le reçoit d'abord, puis le RELAIE vers C (cascade). Chaque nœud
 * vérifie la signature de l'endosseur et intègre l'endossement dans sa
 * réputation locale. Un endossement d'un nœud non de confiance est ignoré
 * (rejeté au gossip ET à l'application), un doublon est ignoré.
 *
 * Promotion : avec `REQUIRED_ENDORSEMENTS = 3` et `ENDORSEMENT_DECAY = 0.5`,
 * un seul endosseur ne peut pas porter B au-dessus de `TRUSTED_THRESHOLD`
 * (0.8 × 0.5 = 0.4 < 0.7). Deux fondateurs de confiance supplémentaires
 * (F1, F2) endossent donc aussi B par le même gossip : après 3 endossements
 * qualifiés, C considère B comme de confiance — uniquement via les
 * endossements reçus.
 */
#[tokio::test]
async fn test_endorsement_propagation_three_nodes() -> Result<(), String> {
    // Le trinôme du brief : A (fondateur de confiance), B (endossé), C (observateur).
    let mut node_a = Node::new(NodeConfig {
        display_name: "A".to_string(),
        ..Default::default()
    });
    let mut node_b = Node::new(NodeConfig {
        display_name: "B".to_string(),
        ..Default::default()
    });
    let mut node_c = Node::new(NodeConfig {
        display_name: "C".to_string(),
        ..Default::default()
    });
    let a_pubkey = node_a.identity.pubkey_hex();
    let b_pubkey = node_b.identity.pubkey_hex();

    // WoT : A est le fondateur de confiance ; B et C le connaissent (le PoW
    // adaptatif d'A est donc 0 chez eux). B n'est PAS encore de confiance.
    node_b.reputation.bootstrap(std::slice::from_ref(&a_pubkey));
    node_c.reputation.bootstrap(std::slice::from_ref(&a_pubkey));
    assert!(!node_c.reputation.is_trusted(&b_pubkey));
    assert_eq!(node_c.reputation.score(&b_pubkey), 0.0);

    // 1. A endosse B : application locale (anti-self/anti-doublon de `endorse`)
    //    + événement Endorsement signé diffusé dans le gossip.
    let event = node_a.endorse(&b_pubkey)?;
    assert!(matches!(event.kind, OndeMessageType::Endorsement));
    assert!(!event.sig.is_empty(), "endorsement must be signed");
    assert_eq!(node_a.reputation.endorsement_count(&b_pubkey), 1);

    // 2. Propagation A → B (direct), puis relai B → C (cascade).
    assert_eq!(gossip_sync(&mut node_a, &mut node_b)?, 1, "B receives the endorsement");
    assert_eq!(node_b.reputation.endorsement_count(&b_pubkey), 1, "B integrates it");

    assert_eq!(gossip_sync(&mut node_b, &mut node_c)?, 1, "B relays the endorsement to C");
    assert_eq!(node_c.reputation.endorsement_count(&b_pubkey), 1, "C integrates it");
    assert!(
        node_c.reputation.score(&b_pubkey) > 0.0,
        "B's reputation must rise above the unknown threshold (0.0)"
    );
    assert!(
        node_c
            .reputation
            .pending_endorsements()
            .iter()
            .any(|e| e.endorser == a_pubkey && e.endorsed == b_pubkey),
        "integrated endorsement must be relayable"
    );

    // 3. Un doublon est ignoré : redélivraison du même événement → déduplication.
    assert_eq!(gossip_sync(&mut node_b, &mut node_c)?, 0, "duplicate event is deduplicated");
    // Et au niveau application : la ré-application du même endossement est rejetée
    // par la logique `endorse` (anti-doublon).
    match node_c.handle_incoming_endorsement(&event) {
        EndorsementHandlingOutcome::Rejected(reason) => {
            assert!(reason.contains("Duplicate"), "duplicate must be rejected: {reason}")
        }
        other => panic!("duplicate endorsement must be rejected, got {other:?}"),
    }
    assert_eq!(node_c.reputation.endorsement_count(&b_pubkey), 1, "no double counting");

    // 4. Un endossement d'un nœud NON de confiance est ignoré.
    let attacker = Identity::generate();
    let evil = Endorsement {
        endorser: attacker.pubkey_hex(),
        endorsed: b_pubkey.clone(),
        timestamp: 1_800_000_000,
    };
    let evil_content = base64::engine::general_purpose::STANDARD
        .encode(serde_json::to_vec(&evil).map_err(|e| e.to_string())?);
    let evil_event =
        MeshEvent::new_signed(&attacker, OndeMessageType::Endorsement, evil_content, vec![]);
    // Au niveau gossip : l'inconnu n'a pas payé le PoW adaptatif requis → rejeté.
    let rep = node_c.reputation.clone();
    assert!(
        node_c.gossip.add_event_with_reputation(evil_event.clone(), &rep).is_err(),
        "unknown endorser without PoW must be refused at the gossip layer"
    );
    // Au niveau application : l'endosseur n'est pas de confiance → ignoré.
    match node_c.handle_incoming_endorsement(&evil_event) {
        EndorsementHandlingOutcome::Rejected(_) => {}
        other => panic!("untrusted endorsement must be ignored, got {other:?}"),
    }
    assert_eq!(node_c.reputation.endorsement_count(&b_pubkey), 1, "nothing applied");
    assert_eq!(node_c.reputation.score(&b_pubkey), 0.4, "reputation unchanged");

    // 5. Promotion : deux autres fondateurs de confiance (F1, F2) endossent B
    //    par le même gossip. Après 3 endossements qualifiés, C considère B
    //    comme de confiance — uniquement via les endossements reçus.
    let mut node_f1 = Node::new(NodeConfig {
        display_name: "F1".to_string(),
        ..Default::default()
    });
    let mut node_f2 = Node::new(NodeConfig {
        display_name: "F2".to_string(),
        ..Default::default()
    });
    let f1_pubkey = node_f1.identity.pubkey_hex();
    let f2_pubkey = node_f2.identity.pubkey_hex();
    node_c
        .reputation
        .bootstrap(&[f1_pubkey.clone(), f2_pubkey.clone()]);

    node_f1.endorse(&b_pubkey)?;
    node_f2.endorse(&b_pubkey)?;
    assert_eq!(gossip_sync(&mut node_f1, &mut node_c)?, 1);
    assert_eq!(gossip_sync(&mut node_f2, &mut node_c)?, 1);

    assert_eq!(node_c.reputation.endorsement_count(&b_pubkey), 3);
    assert!(
        node_c.reputation.is_trusted(&b_pubkey),
        "C must consider B trusted after 3 qualified endorsements received"
    );
    assert!(
        node_c.reputation.score(&b_pubkey) >= TRUSTED_THRESHOLD,
        "B's reputation must rise above the trust threshold"
    );
    Ok(())
}