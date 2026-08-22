/// Commandes Tauri sociales Tuitter/Redit.
use serde::Serialize;
use tauri::State;

use onde_core::social::SocialPlatform;
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

#[tauri::command]
pub async fn social_publish_post(
    social: State<'_, tokio::sync::Mutex<Option<SocialStore>>>,
    identity: State<'_, tokio::sync::Mutex<Option<onde_core::crypto::Identity>>>,
    platform: String,
    title: Option<String>,
    body: String,
    community_slug: Option<String>,
) -> Result<SocialPostView, String> {
    let store = social.lock().await;
    let store = store.as_ref().ok_or("social store not initialized")?;
    let ident = identity.lock().await;
    let ident = ident.as_ref().ok_or("identity not loaded")?;
    let pubkey = ident.pubkey_hex();

    let platform = match platform.as_str() {
        "Tuitter" => SocialPlatform::Tuitter,
        "Redit" => SocialPlatform::Redit,
        other => return Err(format!("unknown platform: {other}")),
    };

    let post = onde_core::social::SocialPost {
        id: uuid_v4(),
        platform,
        author_pubkey: pubkey.clone(),
        title: title.filter(|t| !t.is_empty()),
        body,
        community_slug: community_slug.filter(|s| !s.is_empty()),
        parent_id: None,
        media_urls: vec![],
    };

    post.validate()?;
    store.upsert_user(&pubkey, "", "", "")?;
    store.insert_post(&post)?;

    Ok(SocialPostView {
        id: post.id,
        platform: platform_label_str(platform).to_string(),
        author_pubkey: pubkey,
        author_display_name: String::new(),
        title: post.title,
        body: post.body,
        community_slug: post.community_slug,
        vote_score: 0,
        created_at: 0,
    })
}

#[tauri::command]
pub async fn social_list_posts(
    social: State<'_, tokio::sync::Mutex<Option<SocialStore>>>,
    platform: String,
    community_slug: Option<String>,
    limit: usize,
    offset: usize,
) -> Result<Vec<SocialPostView>, String> {
    let store = social.lock().await;
    let store = store.as_ref().ok_or("social store not initialized")?;
    let platform = match platform.as_str() {
        "Tuitter" => SocialPlatform::Tuitter,
        "Redit" => SocialPlatform::Redit,
        other => return Err(format!("unknown platform: {other}")),
    };

    let rows = store.list_posts(platform, community_slug.as_deref(), None, limit.min(100), offset)?;

    Ok(rows
        .into_iter()
        .map(|r| SocialPostView {
            id: r.id,
            platform: r.platform,
            author_pubkey: r.author_pubkey,
            author_display_name: String::new(),
            title: if r.title.is_empty() { None } else { Some(r.title) },
            body: r.body,
            community_slug: if r.community_slug.is_empty() {
                None
            } else {
                Some(r.community_slug)
            },
            vote_score: r.vote_score,
            created_at: r.created_at,
        })
        .collect())
}

#[tauri::command]
pub async fn social_list_comments(
    social: State<'_, tokio::sync::Mutex<Option<SocialStore>>>,
    post_id: String,
) -> Result<Vec<SocialCommentView>, String> {
    let store = social.lock().await;
    let store = store.as_ref().ok_or("social store not initialized")?;
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
}

#[tauri::command]
pub async fn social_vote(
    social: State<'_, tokio::sync::Mutex<Option<SocialStore>>>,
    identity: State<'_, tokio::sync::Mutex<Option<onde_core::crypto::Identity>>>,
    target_id: String,
    direction: i32,
    target_table: String,
) -> Result<(), String> {
    let store = social.lock().await;
    let store = store.as_ref().ok_or("social store not initialized")?;
    let ident = identity.lock().await;
    let ident = ident.as_ref().ok_or("identity not loaded")?;

    store.vote(&ident.pubkey_hex(), &target_id, direction.clamp(-1, 1), &target_table)
}

#[tauri::command]
pub async fn social_follow(
    social: State<'_, tokio::sync::Mutex<Option<SocialStore>>>,
    identity: State<'_, tokio::sync::Mutex<Option<onde_core::crypto::Identity>>>,
    followed_pubkey: String,
    unfollow: bool,
) -> Result<(), String> {
    let store = social.lock().await;
    let store = store.as_ref().ok_or("social store not initialized")?;
    let ident = identity.lock().await;
    let ident = ident.as_ref().ok_or("identity not loaded")?;

    if unfollow {
        store.unfollow(&ident.pubkey_hex(), &followed_pubkey)
    } else {
        store.follow(&ident.pubkey_hex(), &followed_pubkey)
    }
}

#[tauri::command]
pub async fn social_community_membership(
    social: State<'_, tokio::sync::Mutex<Option<SocialStore>>>,
    identity: State<'_, tokio::sync::Mutex<Option<onde_core::crypto::Identity>>>,
    community_slug: String,
    leave: bool,
) -> Result<(), String> {
    let store = social.lock().await;
    let store = store.as_ref().ok_or("social store not initialized")?;
    let ident = identity.lock().await;
    let ident = ident.as_ref().ok_or("identity not loaded")?;

    if leave {
        store.leave_community(&ident.pubkey_hex(), &community_slug)
    } else {
        store.join_community(&ident.pubkey_hex(), &community_slug)
    }
}

#[tauri::command]
pub async fn social_send_message(
    social: State<'_, tokio::sync::Mutex<Option<SocialStore>>>,
    identity: State<'_, tokio::sync::Mutex<Option<onde_core::crypto::Identity>>>,
    recipient_pubkey: String,
    body: String,
) -> Result<(), String> {
    let store = social.lock().await;
    let store = store.as_ref().ok_or("social store not initialized")?;
    let ident = identity.lock().await;
    let ident = ident.as_ref().ok_or("identity not loaded")?;

    store.insert_message(&uuid_v4(), &ident.pubkey_hex(), &recipient_pubkey, &body)
}

#[tauri::command]
pub async fn social_list_messages(
    social: State<'_, tokio::sync::Mutex<Option<SocialStore>>>,
    identity: State<'_, tokio::sync::Mutex<Option<onde_core::crypto::Identity>>>,
) -> Result<Vec<MessageView>, String> {
    let store = social.lock().await;
    let store = store.as_ref().ok_or("social store not initialized")?;
    let ident = identity.lock().await;
    let ident = ident.as_ref().ok_or("identity not loaded")?;

    let rows = store.list_messages(&ident.pubkey_hex())?;
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
}

fn uuid_v4() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let a: u32 = rng.gen();
    let b: u16 = rng.gen();
    let c: u16 = rng.gen();
    let d: u64 = rng.gen();
    format!(
        "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
        a, b & 0x0fff, c & 0x0fff, (d as u16 & 0x3fff) | 0x8000, d >> 16,
    )
}

fn platform_label_str(p: SocialPlatform) -> &'static str {
    match p {
        SocialPlatform::Tuitter => "Tuitter",
        SocialPlatform::Redit => "Redit",
    }
}

// ── Bookmarks ──

#[tauri::command]
pub async fn social_add_bookmark(
    social: State<'_, tokio::sync::Mutex<Option<SocialStore>>>,
    identity: State<'_, tokio::sync::Mutex<Option<onde_core::crypto::Identity>>>,
    target_id: String,
) -> Result<(), String> {
    let store = social.lock().await;
    let store = store.as_ref().ok_or("social store not initialized")?;
    let ident = identity.lock().await;
    let ident = ident.as_ref().ok_or("identity not loaded")?;
    store.add_bookmark(&ident.pubkey_hex(), &target_id)
}

#[tauri::command]
pub async fn social_remove_bookmark(
    social: State<'_, tokio::sync::Mutex<Option<SocialStore>>>,
    identity: State<'_, tokio::sync::Mutex<Option<onde_core::crypto::Identity>>>,
    target_id: String,
) -> Result<(), String> {
    let store = social.lock().await;
    let store = store.as_ref().ok_or("social store not initialized")?;
    let ident = identity.lock().await;
    let ident = ident.as_ref().ok_or("identity not loaded")?;
    store.remove_bookmark(&ident.pubkey_hex(), &target_id)
}

#[tauri::command]
pub async fn social_list_bookmarks(
    social: State<'_, tokio::sync::Mutex<Option<SocialStore>>>,
    identity: State<'_, tokio::sync::Mutex<Option<onde_core::crypto::Identity>>>,
) -> Result<Vec<String>, String> {
    let store = social.lock().await;
    let store = store.as_ref().ok_or("social store not initialized")?;
    let ident = identity.lock().await;
    let ident = ident.as_ref().ok_or("identity not loaded")?;
    store.list_bookmarks(&ident.pubkey_hex())
}

// ── Post detail ──

#[tauri::command]
pub async fn social_get_post(
    social: State<'_, tokio::sync::Mutex<Option<SocialStore>>>,
    post_id: String,
) -> Result<Option<SocialPostView>, String> {
    let store = social.lock().await;
    let store = store.as_ref().ok_or("social store not initialized")?;
    let row = store.get_post(&post_id)?;
    Ok(row.map(|r| SocialPostView {
        id: r.id,
        platform: r.platform,
        author_pubkey: r.author_pubkey,
        author_display_name: String::new(),
        title: if r.title.is_empty() { None } else { Some(r.title) },
        body: r.body,
        community_slug: if r.community_slug.is_empty() { None } else { Some(r.community_slug) },
        vote_score: r.vote_score,
        created_at: r.created_at,
    }))
}

// ── Moderation ──

#[tauri::command]
pub async fn social_report_post(
    social: State<'_, tokio::sync::Mutex<Option<SocialStore>>>,
    identity: State<'_, tokio::sync::Mutex<Option<onde_core::crypto::Identity>>>,
    target_id: String,
    reason: String,
) -> Result<(), String> {
    let store = social.lock().await;
    let store = store.as_ref().ok_or("social store not initialized")?;
    let ident = identity.lock().await;
    let ident = ident.as_ref().ok_or("identity not loaded")?;
    store.submit_report(&uuid_v4(), &ident.pubkey_hex(), &target_id, &reason)
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
pub async fn social_list_reports(
    social: State<'_, tokio::sync::Mutex<Option<SocialStore>>>,
) -> Result<Vec<ReportView>, String> {
    let store = social.lock().await;
    let store = store.as_ref().ok_or("social store not initialized")?;
    let rows = store.list_open_reports()?;
    Ok(rows.into_iter().map(|r| ReportView {
        id: r.id,
        reporter_pubkey: r.reporter_pubkey,
        target_id: r.target_id,
        reason: r.reason,
        status: r.status,
        created_at: r.created_at,
    }).collect())
}