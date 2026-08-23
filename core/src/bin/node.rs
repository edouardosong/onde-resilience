//! ONDE Node Binary
//!
//! Runnable node daemon with CLI arguments.
//! Usage: onde_node --type mobile --name "MyNode"

use std::env;
use tokio::signal;

use onde_core::health::{spawn_health_server, HealthHandle};
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
                println!(
                    "  --health-port <port>      Serve GET /health JSON on 127.0.0.1:<port>\n                     \x20                           (0 = ephemeral port; disabled by default)"
                );
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

    // Wait for shutdown signal
    tracing::info!("Node running. Press Ctrl+C to stop.");
    signal::ctrl_c().await?;

    tracing::info!("Shutting down...");
    node.stop().await;

    Ok(())
}
