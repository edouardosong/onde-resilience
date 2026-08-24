//! ONDE Node Binary
//!
//! Runnable node daemon with CLI arguments.
//! Usage: onde_node --type mobile --name "MyNode"

use std::env;
use tokio::signal;

use onde_core::health::{spawn_health_server, HealthHandle};
use onde_core::network::tcp::{flush_outbound, process_inbound, TcpTransport, TcpTransportConfig};
use onde_core::node::{Node, NodeConfig, NodeType};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("onde_core=info".parse()?)
                .add_directive("onde_node=info".parse()?),
        )
        .init();

    // Parse simple CLI args
    let args: Vec<String> = env::args().collect();
    let mut node_type = NodeType::Mobile;
    let mut name = format!("onde-node-{}", rand::random::<u16>());
    let mut sqlite_path: Option<String> = None;
    let mut battery_saver = false;
    let mut geohash = String::from("u09tunq"); // position par défaut (démo Paris)
                                               // Phase 3.6 — endpoint de santé désactivé par défaut (opt-in explicite).
    let mut health_port: Option<u16> = None;
    // T32 — transport TCP réel : OFF par défaut (rétro-compatible). Le
    // serveur n'écoute QUE si --listen est passé ; les pairs ne sont joints
    // QUE s'ils sont listés dans --peers.
    let mut tcp_listen: Option<std::net::SocketAddr> = None;
    let mut tcp_peers: Vec<std::net::SocketAddr> = Vec::new();
    // T32-B — démo/ops sur appareils réels : publication unique au démarrage +
    // Web of Trust explicite (mêmes sémantiques que les tests e2e Phase A).
    let mut publish_msg: Option<String> = None;
    let mut trust_pubkeys: Vec<String> = Vec::new();

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--type" => {
                if i + 1 < args.len() {
                    node_type = match args[i + 1].as_str() {
                        "mobile" => NodeType::Mobile,
                        "desktop" => NodeType::DesktopBridge,
                        other => {
                            eprintln!("Unknown node type: {other}. Using mobile.");
                            NodeType::Mobile
                        }
                    };
                    i += 1;
                }
            }
            "--name" => {
                if i + 1 < args.len() {
                    name = args[i + 1].clone();
                    i += 1;
                }
            }
            "--db" => {
                if i + 1 < args.len() {
                    sqlite_path = Some(args[i + 1].clone());
                    i += 1;
                }
            }
            "--battery-saver" => {
                battery_saver = true;
            }
            "--health-port" => {
                if i + 1 < args.len() {
                    match args[i + 1].parse::<u16>() {
                        Ok(port) => health_port = Some(port),
                        Err(_) => {
                            return Err(format!(
                                "--health-port expects a port number (0-65535), got {:?}",
                                args[i + 1]
                            )
                            .into())
                        }
                    }
                    i += 1;
                } else {
                    return Err("--health-port expects a value <port>".to_string().into());
                }
            }
            "--listen" => {
                if i + 1 < args.len() {
                    match args[i + 1].parse::<std::net::SocketAddr>() {
                        Ok(addr) => tcp_listen = Some(addr),
                        Err(_) => {
                            return Err(format!(
                                "--listen expects an ip:port address, got {:?}",
                                args[i + 1]
                            )
                            .into())
                        }
                    }
                    i += 1;
                } else {
                    return Err("--listen expects a value <ip:port>".to_string().into());
                }
            }
            "--peers" => {
                if i + 1 < args.len() {
                    for entry in args[i + 1].split(',') {
                        let entry = entry.trim();
                        if entry.is_empty() {
                            continue;
                        }
                        match entry.parse::<std::net::SocketAddr>() {
                            Ok(addr) => tcp_peers.push(addr),
                            Err(_) => {
                                return Err(format!(
                                    "--peers expects comma-separated ip:port addresses, got {:?}",
                                    entry
                                )
                                .into())
                            }
                        }
                    }
                    i += 1;
                } else {
                    return Err("--peers expects a value <ip:port[,ip:port…]>"
                        .to_string()
                        .into());
                }
            }
            "--publish" => {
                if i + 1 < args.len() {
                    publish_msg = Some(args[i + 1].clone());
                    i += 1;
                } else {
                    return Err("--publish expects a value <message>".to_string().into());
                }
            }
            "--trust" => {
                if i + 1 < args.len() {
                    for entry in args[i + 1].split(',') {
                        let key = entry.trim();
                        if !key.is_empty() {
                            trust_pubkeys.push(key.to_string());
                        }
                    }
                    i += 1;
                } else {
                    return Err("--trust expects a value <hex_pubkey[,hex_pubkey…]>"
                        .to_string()
                        .into());
                }
            }
            "--geohash" => {
                if i + 1 < args.len() {
                    geohash = args[i + 1].clone();
                    i += 1;
                }
            }
            "--help" | "-h" => {
                println!("ONDE Node — Réseau de Résilience Citoyen");
                println!();
                println!("Usage: onde_node [OPTIONS]");
                println!();
                println!("Options:");
                println!("  --type <mobile|desktop>  Node type (default: mobile)");
                println!("  --name <name>            Node display name");
                println!("  --db <path>              SQLite database path (persistence)");
                println!("  --battery-saver          Enable battery saver mode (throttled background work)");
                println!(
                    "  --geohash <geohash>       Node geohash position (7 chars, default: u09tunq)"
                );
                println!("  --health-port <port>     Serve GET /health JSON on 127.0.0.1:<port>");
                println!("                           (0 = ephemeral port; disabled by default)");
                println!(
                    "  --listen <ip:port>       Serveur TCP mesh : accepter les pairs sur <ip:port>"
                );
                println!("                           (ex. 0.0.0.0:9333 ; désactivé par défaut)");
                println!(
                    "  --peers <a,b,c>          Pairs TCP à joindre, séparés par des virgules"
                );
                println!(
                    "                           (ex. 192.168.1.12:9333 ; reconnexion automatique)"
                );
                println!(
                    "  --publish <message>    Publier une alerte signée au démarrage (une fois)"
                );
                println!("  --trust <hex[,hex…]>   Clés publiques de confiance (Web of Trust, bootstrap)");
                println!("  --help, -h               Show this help");
                return Ok(());
            }
            _ => {}
        }
        i += 1;
    }

    let config = NodeConfig {
        node_type,
        display_name: name.clone(),
        available_ram_mb: if node_type == NodeType::DesktopBridge {
            32768
        } else {
            4096
        },
        storage_gb: if node_type == NodeType::DesktopBridge {
            512
        } else {
            64
        },
        ai_model_preference: if node_type == NodeType::DesktopBridge {
            Some("Qwen9B".to_string())
        } else {
            None
        },
        max_peer_connections: if node_type == NodeType::DesktopBridge {
            100
        } else {
            20
        },
        sqlite_path,
        social_db_path: None,
        battery_saver,
        my_geohash: geohash,
        identity_seed: None,
        update_root_pubkey: None,
        update_root_seed: None,
        update_version: String::from("1.0.0"),
    };

    tracing::info!("ONDE Node v0.1.0 starting...");
    tracing::info!(
        "Type: {:?} | Name: {}",
        config.node_type,
        config.display_name
    );

    let mut node = Node::new(config);
    node.start().await?;

    // T32-B — Web of Trust explicit (opt-in) : mêmes sémantiques que le
    // bootstrap des tests e2e Phase A (confiance → difficulté PoW réduite pour
    // les pairs connus, gate complet conservé).
    if !trust_pubkeys.is_empty() {
        node.reputation.bootstrap(&trust_pubkeys);
        tracing::info!("event=trust_bootstrap count={}", trust_pubkeys.len());
    }

    // T32-C — garde opérateur NON bloquante : publier vers des pairs sans
    // Web of Trust explicite est le piège exact de l'incident réel du
    // 2026-08-24. Un receveur qui n'a pas NOTRE clé dans son propre --trust
    // refuse nos alertes en difficulté 0 (plancher PoW adaptatif : auteur
    // inconnu ⇒ MAX_POW_DIFFICULTY) ; la frame part, arrive, puis est rejetée
    // au gate d'admission — invisible côté émetteur sans log. La sémantique
    // DTN (store-and-forward) reste inchangée : on prévient, on n'empêche pas.
    if publish_msg.is_some() && !tcp_peers.is_empty() && trust_pubkeys.is_empty() {
        tracing::warn!(
            "event=publish_without_trust peer_count={} my_pubkey={} hint=listez cette clé dans le --trust de CHAQUE pair visé, sinon ses refus d'admission (PoW adaptatif) écartent l'alerte sans livraison",
            tcp_peers.len(),
            node.identity.pubkey_hex()
        );
    }

    // Phase 3.6 — log structuré UNIQUE de démarrage : snapshot JSON complet
    // (mêmes champs que GET /health), sans aucun secret.
    node.metrics.log_startup_snapshot(&node.config.display_name);

    // Phase 3.6 — endpoint de santé (opt-in : --health-port). Le port 0 =
    // bind éphémère ; le port effectif est loggé. Un échec de bind est fatal
    // et explicite (l'opérateur a demandé cet endpoint).
    let _health: Option<HealthHandle> = match health_port {
        Some(port) => match spawn_health_server(port, node.metrics.clone()) {
            Ok(handle) => {
                tracing::info!(
                    "event=health_listen addr=127.0.0.1:{} path=/health",
                    handle.port
                );
                Some(handle)
            }
            Err(e) => return Err(format!("health endpoint bind failed on port {port}: {e}").into()),
        },
        None => None,
    };

    // Print status
    let status = node.status().await;
    tracing::info!("Node status: {:#?}", status);

    // T32 — transport TCP réel (opt-in : --listen / --peers). Sans ces
    // flags, le comportement est identique à l'avant-T32 (attente Ctrl+C
    // passive). Avec : serveur borné + fils clients + pump gossip ⇆ TCP sur
    // ce fil (le Node n'est pas partagé entre fils — même modèle que les
    // tests e2e). Une frame TCP suit EXACTEMENT le chemin d'un événement de
    // pair gossip : from_wire_bytes → gate d'admission → handlers métier.
    let transport: Option<TcpTransport> = if tcp_listen.is_none() && tcp_peers.is_empty() {
        None
    } else {
        let config = TcpTransportConfig {
            listen: tcp_listen,
            peers: tcp_peers,
            ..TcpTransportConfig::default()
        };
        let transport = TcpTransport::new(config);
        // Bind demandé explicitement par l'opérateur → échec = fatal explicite.
        transport
            .start()
            .map_err(|e| format!("tcp mesh transport start failed: {e}"))?;
        if let Some(addr) = transport.listen_addr() {
            tracing::info!("event=tcp_mesh_listen addr={addr}");
        }
        tracing::info!("event=tcp_mesh_peers count={}", transport.peer_keys().len());
        Some(transport)
    };

    // T32-B — publication unique au démarrage (opt-in). L'événement entre dans
    // les broadcasts pending du gossip : il sera livré à chaque pair qui se
    // connectera ensuite (store-and-forward, même sémantique que DTN).
    if let Some(msg) = publish_msg {
        match node.publish_alert(msg).await {
            Ok(event) => tracing::info!(
                "event=alert_published id={} kind={:?} signature_valid={} pubkey={}",
                event.id,
                event.kind,
                event.signature_valid(),
                node.identity.pubkey_hex()
            ),
            Err(e) => return Err(format!("publish failed: {e}").into()),
        }
    }

    const PUMP_INTERVAL: std::time::Duration = std::time::Duration::from_millis(200);
    match &transport {
        None => {
            // Comportement historique inchangé.
            tracing::info!("Node running. Press Ctrl+C to stop.");
            signal::ctrl_c().await?;
        }
        Some(transport) => {
            tracing::info!("Node running with TCP mesh transport. Press Ctrl+C to stop.");
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(PUMP_INTERVAL) => {
                        let outbound = flush_outbound(&mut node, transport);
                        let inbound = process_inbound(&mut node, transport);
                        if outbound.frames_queued_outbound > 0 {
                            tracing::debug!(
                                "event=tcp_pump_outbound frames={}",
                                outbound.frames_queued_outbound
                            );
                        }
                        if inbound.frames_received > 0 {
                            tracing::debug!(
                                "event=tcp_pump_inbound received={} ingested={} rejected={}",
                                inbound.frames_received,
                                inbound.events_ingested,
                                inbound.events_rejected
                            );
                        }
                    }
                    _ = signal::ctrl_c() => break,
                }
            }
        }
    }

    if let Some(transport) = &transport {
        transport.stop();
        tracing::info!(
            "event=tcp_mesh_stats sent={} received={}",
            transport.stats().frames_sent,
            transport.stats().frames_received
        );
    }

    tracing::info!("Shutting down...");
    node.stop().await;

    Ok(())
}
