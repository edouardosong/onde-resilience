//! T32-C — course `--publish` ↔ premier dial TCP : preuve e2e déterministe.
//!
//! Reproduit EXACTEMENT l'ordre du binaire `onde_node` (core/src/bin/node.rs) :
//! 1. `transport.start()` — le dial vers le pair démarre IMMÉDIATEMENT,
//! 2. `node.publish_alert(..)` — AVANT toute passe de pump,
//! 3. boucle de pump (flush_outbound puis process_inbound) jusqu'à stabilisation.
//!
//! Garantie testée : une alerte publiée AVANT l'établissement de la connexion
//! est livrée au pair après stabilisation (pump borné) — la fenêtre
//! « publish avant dial » ne doit rien perdre côté transport/gossip.
//!
//! Deux scénarios :
//! - Scénario A (confiance symétrique, comme les tests Phase A) : livraison
//!   intégrale — mémoire tier Critical + SQLite + compteur ingested.
//! - Scénario B (receveur SANS `--trust` émetteur — conditions de l'incident
//!   réel du 2026-08-24) : la frame traverse le réseau (sent/received, zéro
//!   violation protocole) mais est REFUSÉE par le gate d'admission (plancher
//!   PoW adaptatif : auteur auto-confiant publie en difficulté 0, receveur
//!   exige MAX_POW_DIFFICULTY pour un inconnu). Le refus doit être OBSERVABLE
//!   (`PumpReport::events_rejected >= 1`) — jamais un silence total.
//!
//! CI-safe : localhost, ports éphémères OS, boucles bornées.

use std::time::{Duration, Instant};

use onde_core::network::tcp::{flush_outbound, process_inbound, TcpTransport, TcpTransportConfig};
use onde_core::node::{Node, NodeConfig, NodeType};

/// Config miroir du binaire : B est CLIENT seul (`--peers` sans `--listen`),
/// budgets courts pour un arrêt propre rapide.
fn binary_like_client_config(peer: std::net::SocketAddr) -> TcpTransportConfig {
    TcpTransportConfig {
        listen: None,
        peers: vec![peer],
        max_connections: 16,
        reconnect_interval: Duration::from_millis(100),
        connect_timeout: Duration::from_secs(2),
        socket_timeout: Duration::from_millis(300),
        queue_capacity: 64,
    }
}

fn listener_config() -> TcpTransportConfig {
    TcpTransportConfig {
        listen: Some("127.0.0.1:0".parse().expect("valid test address")),
        peers: Vec::new(),
        max_connections: 16,
        reconnect_interval: Duration::from_millis(100),
        connect_timeout: Duration::from_secs(2),
        socket_timeout: Duration::from_millis(300),
        queue_capacity: 64,
    }
}

fn test_node(name: &str, db_path: &str) -> Node {
    Node::new(NodeConfig {
        node_type: NodeType::Mobile,
        display_name: name.to_string(),
        available_ram_mb: 4096,
        storage_gb: 64,
        sqlite_path: Some(db_path.to_string()),
        ..NodeConfig::default()
    })
}

/// Scénario A — publication AVANT le dial, confiance symétrique :
/// livraison garantie après stabilisation.
#[tokio::test]
async fn publish_before_dial_is_delivered_after_settlement() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut node_a = test_node("NodeA-T32C", &format!("{}/a.sqlite", dir.path().display()));
    let mut node_b = test_node("NodeB-T32C", &format!("{}/b.sqlite", dir.path().display()));

    // Confiance symétrique (comme l'e2e Phase A) : A fait confiance à B —
    // l'alerte de B (difficulté 0, auto-confiance GENESIS_TRUST) passe le
    // plancher PoW adaptatif de A.
    let b_pubkey = node_b.identity.pubkey_hex().to_string();
    node_a.reputation.bootstrap(std::slice::from_ref(&b_pubkey));

    let transport_a = TcpTransport::new(listener_config());
    transport_a.start().expect("bind A");
    let addr_a = transport_a.listen_addr().expect("A bound");

    // ── Ordre EXACT du binaire : dial démarré AVANT le publish ──────────
    let transport_b = TcpTransport::new(binary_like_client_config(addr_a));
    transport_b.start().expect("client B starts dial");

    // Publish immédiat : le dial est EN COURS (voire déjà refusé/retenté),
    // aucune passe de pump n'a encore tourné. C'est la course visée.
    let published = node_b
        .publish_alert("T32-C: publish avant dial — livraison garantie".to_string())
        .await
        .expect("publish must succeed (self-trusted, difficulty 0)");

    // ── Boucle de pump bornée (miroir du binaire : out puis in) ─────────
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut delivered = false;
    let mut last_report_in = None;
    let mut last_report_out = None;
    while Instant::now() < deadline {
        last_report_out = Some(flush_outbound(&mut node_b, &transport_b));
        last_report_in = Some(process_inbound(&mut node_a, &transport_a));
        if node_a.message_store.get(&published.id).is_some()
            && node_a.metrics.snapshot().metrics.messages_ingested >= 1
            && node_a
                .persistence
                .as_ref()
                .expect("sqlite configured")
                .get(&published.id)
                .expect("sqlite read")
                .is_some()
        {
            delivered = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    assert!(
        delivered,
        "alerte publiée AVANT le dial doit être stockée (mémoire+SQLite) \
         chez A après stabilisation (in={last_report_in:?} out={last_report_out:?})"
    );
    assert_eq!(
        transport_b.stats().protocol_violations,
        0,
        "échange légitime : zéro violation protocole"
    );

    transport_a.stop();
    transport_b.stop();
}

/// Scénario B — conditions exactes de l'incident réel (receveur sans
/// `--trust` émetteur) : la frame PART et ARRIVE, mais le gate d'admission
/// refuse (plancher PoW adaptatif) ; le refus doit laisser une trace
/// observable dans le bilan de pump — jamais un silence total.
///
/// Ce test CARACTÉRISE le mécanisme de perte observé sur appareils
/// (2026-08-24) : perte d'ADMISSION silencieuse, pas de race transport.
#[tokio::test]
async fn untrusted_receiver_rejects_difficulty_zero_alert_visibly() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut node_a = test_node(
        "NodeA-UNTRUSTED",
        &format!("{}/a.sqlite", dir.path().display()),
    );
    let mut node_b = test_node(
        "NodeB-PUBLISHER",
        &format!("{}/b.sqlite", dir.path().display()),
    );
    // PAS de bootstrap de B chez A — c'est le point de l'incident.

    let transport_a = TcpTransport::new(listener_config());
    transport_a.start().expect("bind A");
    let addr_a = transport_a.listen_addr().expect("A bound");

    let transport_b = TcpTransport::new(binary_like_client_config(addr_a));
    transport_b.start().expect("client B starts dial");

    let published = node_b
        .publish_alert("T32-C: incident shape — receiver lacks trust".to_string())
        .await
        .expect("publish succeeds locally (self-trust)");

    // Preuve locale : B a bien publié en difficulté 0 (auto-confiance WoT).
    assert_eq!(
        published.pow_difficulty, 0,
        "nœud auto-confié : difficulté PoW 0 attendue"
    );

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut saw_frame_cross = false;
    let mut last_report_in = None;
    while Instant::now() < deadline {
        let _out = flush_outbound(&mut node_b, &transport_b);
        let rep_in = process_inbound(&mut node_a, &transport_a);
        last_report_in = Some(rep_in);
        // La frame a traversé le réseau dans tous les cas : sent ET received.
        if transport_b.stats().frames_sent >= 1 && transport_a.stats().frames_received >= 1 {
            saw_frame_cross = true;
            if rep_in.frames_received > 0 {
                break; // frame traitée par le pump de A — verdict disponible
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(saw_frame_cross, "la frame doit partir de B et arriver à A");

    let stats_a = transport_a.stats();
    assert_eq!(
        stats_a.protocol_violations, 0,
        "la frame est un wire VALIDE : aucune violation framing — \
         la perte ne peut venir que du gate d'admission"
    );

    // Verdict : NON stockée (mémoire ET SQLite) mais refus OBSERVABLE.
    assert!(
        node_a.message_store.get(&published.id).is_none(),
        "sans confiance, l'alerte difficulté 0 doit être refusée (sécurité préservée)"
    );
    let persisted = node_a
        .persistence
        .as_ref()
        .expect("sqlite configured")
        .get(&published.id)
        .expect("sqlite read");
    assert!(persisted.is_none(), "rien en SQLite non plus");

    let rep = last_report_in.expect("au moins une passe de pump avec frame");
    assert!(
        rep.events_rejected >= 1,
        "le refus d'admission DOIT être visible dans le bilan de pump \
         (events_rejected) — c'est l'antidote au « silence total » de l'incident \
         (report={rep:?})"
    );

    transport_a.stop();
    transport_b.stop();
}
