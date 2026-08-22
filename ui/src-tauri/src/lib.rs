/// ONDE UI Library — Tauri app setup
mod commands;
mod social_commands;

use commands::AppState;
use social_commands::{
    social_add_bookmark, social_community_membership, social_follow, social_get_post,
    social_list_bookmarks, social_list_comments, social_list_messages, social_list_posts,
    social_list_reports, social_publish_post, social_remove_bookmark, social_report_post,
    social_send_message, social_vote,
};

// T13-checker M1 : plus d'états sociaux fantômes — les commandes sociales
// passent par le Node réel de `commands::AppState` (identité stable + cache
// SQLite dédié ouverts à `node_start`).
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            commands::node_start,
            commands::node_stop,
            commands::node_status,
            commands::publish_alert,
            commands::publish_mutual_aid,
            commands::get_feed_events,
            commands::get_reputation,
            commands::demo_identity,
            social_publish_post,
            social_list_posts,
            social_get_post,
            social_list_comments,
            social_vote,
            social_follow,
            social_community_membership,
            social_send_message,
            social_list_messages,
            social_add_bookmark,
            social_remove_bookmark,
            social_list_bookmarks,
            social_report_post,
            social_list_reports,
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
