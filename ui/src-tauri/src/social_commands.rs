/// Commandes Tauri sociales Tuitter/Redit (T13 Fusion).
///
/// T13-checker M1 — CHOIX DE CÂBLAGE RÉEL : toutes les commandes passent par
/// le [`Node`] démarré (`AppState.node`, créé par `node_start`) et donc par
/// SON identité stable et SON `SocialStore` dédié (`social_db_path` ouvert à
/// `node_start`). Plus d'états fantômes jamais initialisés : sans nœud
/// démarré, les commandes renvoient une erreur explicite, comme
/// `publish_alert`/`get_feed_events`.
///
/// Ce qui est propagé dans le mesh aujourd'hui : publications Tuitter/Redit
/// (`social_publish_post` → `Node::publish_social_post`, événement signé
/// kind 16) et commentaires (`Node::publish_social_comment`, kind 17), via le
/// gate d'admission. Les votes/abonnements/messages/bookmarks/signalements
/// restent LOCAUX au cache (leurs kinds 18..21 existent côté core, leur
/// émission UI est un pas suivant documenté).
use serde::Serialize;
use tauri::State;

use crate::commands::AppState;
use onde_core::node::Node;
use onde_core::social::{SocialPlatform, SocialPost};
use onde_core::social_store::SocialStore;

#[derive(Debug, Serialize)]
pub struct SocialPostView {
    pub id: String,
    pub platform: String,
    pub author_pubkey: String,
    pub author_display_name: String,
    pub title: Option<String>,
    pub body: String,
    pub community_slug: Option<String>,
    pub vote_score: i64,
    pub created_at: i64,
}

#[derive(Debug, Serialize)]
pub struct SocialCommentView {
    pub id: String,
    pub platform: String,
    pub author_pubkey: String,
    pub post_id: String,
    pub parent_id: Option<String>,
    pub body: String,
    pub vote_score: i64,
    pub created_at: i64,
}

#[derive(Debug, Serialize)]
pub struct MessageView {
    pub id: String,
    pub sender_pubkey: String,
    pub recipient_pubkey: String,
    pub body: String,
    pub read_at: Option<i64>,
    pub created_at: i64,
}

/// Verrouille le nœud démarré et lui délègue l'opération.
async fn with_node<T>(
    state: State<'_, AppState>,
    f: impl FnOnce(&mut Node) -> Result<T, String>,
) -> Result<T, String> {
    let mut guard = state.node.lock().await;
    let node = guard
        .as_mut()
        .ok_or("Node not started — call node_start first")?;
    f(node)
}

/// Accède au cache social du nœud démarré.
fn social_store_of(node: &Node) -> Result<&SocialStore, String> {
    node.social_store
        .as_ref()
        .ok_or_else(|| "social store unavailable on this node".to_string())
}

fn parse_platform(platform: &str) -> Result<SocialPlatform, String> {
    match platform {
        "Tuitter" => Ok(SocialPlatform::Tuitter),
        "Redit" => Ok(SocialPlatform::Redit),
        other => Err(format!("unknown platform: {other}")),
    }
}

#[tauri::command]
pub async fn social_publish_post(
    state: State<'_, AppState>,
    platform: String,
    title: Option<String>,
    body: String,
    community_slug: Option<String>,
) -> Result<SocialPostView, String> {
    // CÂBLAGE MESH RÉEL : validation + signature Ed25519 + PoW adaptatif +
    // gossip + cache local en une seule opération noyau.
    with_node(state, move |node| {
        let platform = parse_platform(&platform)?;
        let event = node.publish_social_post(
            platform_label(platform),
            title.as_deref(),
            &body,
            community_slug.as_deref(),
        )?;
        let stored: SocialPost =
            serde_json::from_str(&event.content).map_err(|e| format!("post decode: {e}"))?;
        Ok(SocialPostView {
            id: stored.id,
            platform: platform_label(stored.platform).to_string(),
            author_pubkey: stored.author_pubkey,
            author_display_name: node.config.display_name.clone(),
            title: stored.title,
            body: stored.body,
            community_slug: stored.community_slug,
            vote_score: 0,
            created_at: event.created_at as i64,
        })
    })
    .await
}

#[tauri::command]
pub async fn social_list_posts(
    state: State<'_, AppState>,
    platform: String,
    community_slug: Option<String>,
    limit: usize,
    offset: usize,
) -> Result<Vec<SocialPostView>, String> {
    with_node(state, move |node| {
        let store = social_store_of(node)?;
        let platform = parse_platform(&platform)?;
        let rows = store.list_posts(
            platform,
            community_slug.as_deref(),
            None,
            limit.min(100),
            offset,
        )?;
        Ok(rows
            .into_iter()
            .map(|r| {
                let author_display_name = display_name_of(node, &r.author_pubkey);
                SocialPostView {
                    id: r.id,
                    platform: r.platform,
                    author_pubkey: r.author_pubkey,
                    author_display_name,
                    title: if r.title.is_empty() {
                        None
                    } else {
                        Some(r.title)
                    },
                    body: r.body,
                    community_slug: if r.community_slug.is_empty() {
                        None
                    } else {
                        Some(r.community_slug)
                    },
                    vote_score: r.vote_score,
                    created_at: r.created_at,
                }
            })
            .collect())
    })
    .await
}

#[tauri::command]
pub async fn social_list_comments(
    state: State<'_, AppState>,
    post_id: String,
) -> Result<Vec<SocialCommentView>, String> {
    with_node(state, move |node| {
        let store = social_store_of(node)?;
        let rows = store.list_comments(&post_id)?;
        Ok(rows
            .into_iter()
            .map(|r| SocialCommentView {
                id: r.id,
                platform: r.platform,
                author_pubkey: r.author_pubkey,
                post_id: r.post_id,
                parent_id: r.parent_id,
                body: r.body,
                vote_score: r.vote_score,
                created_at: r.created_at,
            })
            .collect())
    })
    .await
}

#[tauri::command]
pub async fn social_vote(
    state: State<'_, AppState>,
    target_id: String,
    direction: i32,
    target_table: String,
) -> Result<(), String> {
    with_node(state, move |node| {
        let store = social_store_of(node)?;
        let voter = node.identity.pubkey_hex();
        store.vote(&voter, &target_id, direction.clamp(-1, 1), &target_table)?;
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn social_follow(
    state: State<'_, AppState>,
    followed_pubkey: String,
    unfollow: bool,
) -> Result<(), String> {
    with_node(state, move |node| {
        let store = social_store_of(node)?;
        let follower = node.identity.pubkey_hex();
        if unfollow {
            store.unfollow(&follower, &followed_pubkey)
        } else {
            store.follow(&follower, &followed_pubkey)
        }
    })
    .await
}

#[tauri::command]
pub async fn social_community_membership(
    state: State<'_, AppState>,
    community_slug: String,
    leave: bool,
) -> Result<(), String> {
    with_node(state, move |node| {
        let store = social_store_of(node)?;
        let pubkey = node.identity.pubkey_hex();
        if leave {
            store.leave_community(&pubkey, &community_slug)
        } else {
            store.join_community(&pubkey, &community_slug)
        }
    })
    .await
}

#[tauri::command]
pub async fn social_send_message(
    state: State<'_, AppState>,
    recipient_pubkey: String,
    body: String,
) -> Result<(), String> {
    with_node(state, move |node| {
        let store = social_store_of(node)?;
        let sender = node.identity.pubkey_hex();
        store.insert_message(&uuid_v4(), &sender, &recipient_pubkey, &body)
    })
    .await
}

#[tauri::command]
pub async fn social_list_messages(state: State<'_, AppState>) -> Result<Vec<MessageView>, String> {
    with_node(state, move |node| {
        let store = social_store_of(node)?;
        let me = node.identity.pubkey_hex();
        let rows = store.list_messages(&me)?;
        Ok(rows
            .into_iter()
            .map(|r| MessageView {
                id: r.id,
                sender_pubkey: r.sender_pubkey,
                recipient_pubkey: r.recipient_pubkey,
                body: r.body,
                read_at: r.read_at,
                created_at: r.created_at,
            })
            .collect())
    })
    .await
}

// ── Bookmarks ──

#[tauri::command]
pub async fn social_add_bookmark(
    state: State<'_, AppState>,
    target_id: String,
) -> Result<(), String> {
    with_node(state, move |node| {
        let store = social_store_of(node)?;
        let pubkey = node.identity.pubkey_hex();
        store.add_bookmark(&pubkey, &target_id)
    })
    .await
}

#[tauri::command]
pub async fn social_remove_bookmark(
    state: State<'_, AppState>,
    target_id: String,
) -> Result<(), String> {
    with_node(state, move |node| {
        let store = social_store_of(node)?;
        let pubkey = node.identity.pubkey_hex();
        store.remove_bookmark(&pubkey, &target_id)
    })
    .await
}

#[tauri::command]
pub async fn social_list_bookmarks(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    with_node(state, move |node| {
        let store = social_store_of(node)?;
        let pubkey = node.identity.pubkey_hex();
        store.list_bookmarks(&pubkey)
    })
    .await
}

// ── Post detail ──

#[tauri::command]
pub async fn social_get_post(
    state: State<'_, AppState>,
    post_id: String,
) -> Result<Option<SocialPostView>, String> {
    with_node(state, move |node| {
        let store = social_store_of(node)?;
        let row = store.get_post(&post_id)?;
        Ok(row.map(|r| SocialPostView {
            id: r.id,
            platform: r.platform,
            author_pubkey: r.author_pubkey.clone(),
            author_display_name: display_name_of(node, &r.author_pubkey),
            title: if r.title.is_empty() {
                None
            } else {
                Some(r.title)
            },
            body: r.body,
            community_slug: if r.community_slug.is_empty() {
                None
            } else {
                Some(r.community_slug)
            },
            vote_score: r.vote_score,
            created_at: r.created_at,
        }))
    })
    .await
}

// ── Moderation ──

#[tauri::command]
pub async fn social_report_post(
    state: State<'_, AppState>,
    target_id: String,
    reason: String,
) -> Result<(), String> {
    with_node(state, move |node| {
        let store = social_store_of(node)?;
        let reporter = node.identity.pubkey_hex();
        store.submit_report(&uuid_v4(), &reporter, &target_id, &reason)
    })
    .await
}

#[derive(Debug, Serialize)]
pub struct ReportView {
    pub id: String,
    pub reporter_pubkey: String,
    pub target_id: String,
    pub reason: String,
    pub status: String,
    pub created_at: i64,
}

#[tauri::command]
pub async fn social_list_reports(state: State<'_, AppState>) -> Result<Vec<ReportView>, String> {
    with_node(state, move |node| {
        let store = social_store_of(node)?;
        let rows = store.list_open_reports()?;
        Ok(rows
            .into_iter()
            .map(|r| ReportView {
                id: r.id,
                reporter_pubkey: r.reporter_pubkey,
                target_id: r.target_id,
                reason: r.reason,
                status: r.status,
                created_at: r.created_at,
            })
            .collect())
    })
    .await
}

fn uuid_v4() -> String {
    use rand::Rng;
    let rng = &mut rand::thread_rng();
    let a: u32 = rng.gen();
    let b: u16 = rng.gen();
    let c: u16 = rng.gen();
    let d: u64 = rng.gen();
    format!(
        "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
        a,
        b & 0x0fff,
        c & 0x0fff,
        (d as u16 & 0x3fff) | 0x8000,
        d >> 16,
    )
}

fn platform_label(p: SocialPlatform) -> &'static str {
    match p {
        SocialPlatform::Tuitter => "Tuitter",
        SocialPlatform::Redit => "Redit",
    }
}

/// Nom d'affichage connu localement pour un auteur (cache utilisateurs) ;
/// chaîne vide si inconnu — JAMAIS de réinitialisation de profil ici.
fn display_name_of(node: &Node, pubkey: &str) -> String {
    node.social_store
        .as_ref()
        .and_then(|s| s.get_user(pubkey).ok())
        .flatten()
        .map(|u| u.display_name)
        .unwrap_or_default()
}
