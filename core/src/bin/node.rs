//! ONDE Node Binary
//! 
//! Runnable node daemon with CLI arguments.
//! Usage: onde_node --type mobile --name "MyNode"

use std::env;
use tokio::signal;

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
            "--help" | "-h" => {
                println!("ONDE Node — Réseau de Résilience Citoyen");
                println!();
                println!("Usage: onde_node [OPTIONS]");
                println!();
                println!("Options:");
                println!("  --type <mobile|desktop>  Node type (default: mobile)");
                println!("  --name <name>            Node display name");
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
        available_ram_mb: if node_type == NodeType::DesktopBridge { 32768 } else { 4096 },
        storage_gb: if node_type == NodeType::DesktopBridge { 512 } else { 64 },
        ai_model_preference: if node_type == NodeType::DesktopBridge {
            Some("Qwen9B".to_string())
        } else {
            None
        },
        max_peer_connections: if node_type == NodeType::DesktopBridge { 100 } else { 20 },
    };

    tracing::info!("ONDE Node v0.1.0 starting...");
    tracing::info!("Type: {:?} | Name: {}", config.node_type, config.display_name);

    let mut node = Node::new(config);
    node.start().await?;

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