/// Commandes Tauri — bridge entre l'UI et le noyau ONDE.
///
/// Les commandes sont dans un sous-module (et non à la racine du crate) :
/// évite le conflit de macro `#[tauri::command]` sur Rust récent
/// (E0255 — `__cmd_*` défini et ré-importé dans le même scope).
use serde::Serialize;
use tauri::{Manager, State};
use tokio::sync::Mutex;

use onde_core::crypto::Identity;
use onde_core::node::{Node, NodeConfig, NodeType};
use onde_core::protocol::MeshEvent;
use onde_core::reputation::ReputationSystem;

/// État applicatif partagé entre les commandes Tauri.
#[derive(Default)]
pub struct AppState {
    pub node: Mutex<Option<Node>>,
    /// Réputation du réseau (WoT) — partagée entre les nœuds locaux.
    pub reputation: Mutex<ReputationSystem>,
}

/// Résumé d'un événement du flux public (sérialisable vers le frontend).
#[derive(Serialize)]
pub struct FeedEventView {
    pub id: String,
    pub pubkey: String,
    pub created_at: u64,
    pub kind: String,
    pub content: String,
}

fn event_to_view(e: &MeshEvent) -> FeedEventView {
    FeedEventView {
        id: e.id.clone(),
        pubkey: e.pubkey.clone(),
        created_at: e.created_at,
        kind: format!("{:?}", e.kind),
        content: e.content.clone(),
    }
}

/// Démarrer un nœud ONDE local (type mobile par défaut).
///
/// La persistance SQLite est activée avec la base dans le répertoire de
/// données applicatives de la plateforme (`app_data_dir`) — résilience aux
/// crashs (Audit #14).
#[tauri::command]
pub async fn node_start(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    display_name: String,
) -> Result<String, String> {
    let mut guard = state.node.lock().await;
    if guard.is_some() {
        return Err("Node already running".to_string());
    }
    // Répertoire de données applicatives (varie par plateforme) ; s'il n'est
    // pas disponible, le nœud démarre en mémoire seule.
    let sqlite_path = app.path().app_data_dir().ok().map(|dir| {
        let _ = std::fs::create_dir_all(&dir);
        dir.join("onde-mobile.sqlite3")
            .to_string_lossy()
            .to_string()
    });
    let config = NodeConfig {
        node_type: NodeType::Mobile,
        display_name,
        sqlite_path,
        ..Default::default()
    };
    let mut node = Node::new(config);
    // Restaure les messages persistés après un redémarrage (best-effort)
    let _ = node.load_persisted_messages();
    // La réputation locale du nœud est reliée au WoT partagé
    {
        let rep = state.reputation.lock().await;
        node.reputation = rep.clone();
    }
    node.start().await?;
    let pubkey = node.identity.pubkey_hex();
    *guard = Some(node);
    Ok(pubkey)
}

/// Arrêter le nœud local.
#[tauri::command]
pub async fn node_stop(state: State<'_, AppState>) -> Result<(), String> {
    let mut guard = state.node.lock().await;
    if let Some(node) = guard.as_mut() {
        node.stop().await;
    }
    *guard = None;
    Ok(())
}

/// Statut courant du nœud.
#[tauri::command]
pub async fn node_status(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let guard = state.node.lock().await;
    match guard.as_ref() {
        Some(node) => {
            let status = node.status().await;
            Ok(serde_json::to_value(status).map_err(|e| e.to_string())?)
        }
        None => Ok(serde_json::json!({ "is_running": false })),
    }
}

/// Publier une alerte civique (280 caractères max).
#[tauri::command]
pub async fn publish_alert(
    state: State<'_, AppState>,
    content: String,
) -> Result<String, String> {
    let mut guard = state.node.lock().await;
    let node = guard
        .as_mut()
        .ok_or("Node not started — call node_start first")?;
    let event = node.publish_alert(content).await?;
    // Met à jour la réputation partagée après activité
    let mut rep = state.reputation.lock().await;
    rep.record_activity(&event.pubkey);
    Ok(event.id)
}

/// Récupérer le flux public (événements en attente de diffusion).
#[tauri::command]
pub async fn get_feed_events(state: State<'_, AppState>) -> Result<Vec<FeedEventView>, String> {
    let guard = state.node.lock().await;
    let node = guard.as_ref().ok_or("Node not started")?;
    Ok(node
        .gossip
        .get_pending_broadcasts()
        .into_iter()
        .map(event_to_view)
        .collect())
}

/// Résumé de la réputation du réseau (Web of Trust).
#[tauri::command]
pub async fn get_reputation(
    state: State<'_, AppState>,
) -> Result<Vec<(String, f64, usize)>, String> {
    let rep = state.reputation.lock().await;
    Ok(rep.summary())
}

/// Publier une demande d'entraide.
#[tauri::command]
pub async fn publish_mutual_aid(
    state: State<'_, AppState>,
    content: String,
) -> Result<String, String> {
    let mut guard = state.node.lock().await;
    let node = guard
        .as_mut()
        .ok_or("Node not started — call node_start first")?;
    let event = node.publish_mutual_aid(content).await?;
    Ok(event.id)
}

/// Générer une identité de démonstration (jamais stockée — usage UI).
#[tauri::command]
pub async fn demo_identity() -> Result<(String, String), String> {
    let id = Identity::generate();
    Ok((id.pubkey_hex(), id.x25519_public_key_hex()))
}