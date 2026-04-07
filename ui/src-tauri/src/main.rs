#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            tracing::info!("ONDE UI v0.1.0 starting");

            // Log node info
            let status = futures::executor::block_on(async {
                let config = onde_core::node::NodeConfig::default();
                let node = onde_core::node::Node::new(config);
                node.status().await
            });
            tracing::info!("Default node status: {:#?}", status);

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri");
}