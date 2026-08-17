/// ONDE UI Library — Tauri app setup
mod commands;

use commands::{AppState, node_start, node_stop, node_status, publish_alert, publish_mutual_aid, get_feed_events, get_reputation, demo_identity};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            node_start,
            node_stop,
            node_status,
            publish_alert,
            publish_mutual_aid,
            get_feed_events,
            get_reputation,
            demo_identity,
        ])
        .setup(|app| {
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}