/// Persistance SQLite du graphe social partagé Tuitter/Redit.
///
/// Chaque nœud stocke localement ses données sociales ; le gossip ONDE assure
/// la réplication entre nœuds (les événements signés sont les sources de
/// vérité). Ce store est un cache matérialisé, pas une autorité.
use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension};

use crate::social::{SocialComment, SocialPlatform, SocialPost};

const CURRENT_VERSION: u32 = 1;

pub struct SocialStore {
    conn: Mutex<Connection>,
}

impl SocialStore {
    pub fn open(path: &str) -> Result<Self, String> {
        let conn = Connection::open(path).map_err(|e| format!("social db open failed: {e}"))?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA foreign_keys=ON;",
        )
        .map_err(|e| format!("social pragma failed: {e}"))?;
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.migrate()?;
        Ok(store)
    }

    pub fn open_in_memory() -> Result<Self, String> {
        Self::open(":memory:")
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>, String> {
        self.conn
            .lock()
            .map_err(|_| "social db mutex poisoned".to_string())
    }

    fn migrate(&self) -> Result<(), String> {
        let conn = self.lock()?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS social_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS social_users (
                 pubkey TEXT PRIMARY KEY,
                 display_name TEXT NOT NULL DEFAULT '',
                 bio TEXT NOT NULL DEFAULT '',
                 avatar_url TEXT NOT NULL DEFAULT '',
                 created_at INTEGER NOT NULL DEFAULT (unixepoch())
             );
             CREATE TABLE IF NOT EXISTS social_posts (
                 id TEXT PRIMARY KEY,
                 platform TEXT NOT NULL CHECK (platform IN ('Tuitter', 'Redit')),
                 author_pubkey TEXT NOT NULL REFERENCES social_users(pubkey),
                 title TEXT,
                 body TEXT NOT NULL,
                 community_slug TEXT,
                 parent_id TEXT,
                 media_urls TEXT NOT NULL DEFAULT '[]',
                 vote_score INTEGER NOT NULL DEFAULT 0,
                 created_at INTEGER NOT NULL DEFAULT (unixepoch()),
                 updated_at INTEGER NOT NULL DEFAULT (unixepoch())
             );
             CREATE TABLE IF NOT EXISTS social_comments (
                 id TEXT PRIMARY KEY,
                 platform TEXT NOT NULL CHECK (platform IN ('Tuitter', 'Redit')),
                 author_pubkey TEXT NOT NULL REFERENCES social_users(pubkey),
                 post_id TEXT NOT NULL REFERENCES social_posts(id) ON DELETE CASCADE,
                 parent_id TEXT REFERENCES social_comments(id),
                 body TEXT NOT NULL,
                 vote_score INTEGER NOT NULL DEFAULT 0,
                 created_at INTEGER NOT NULL DEFAULT (unixepoch())
             );
             CREATE TABLE IF NOT EXISTS social_votes (
                 voter_pubkey TEXT NOT NULL,
                 target_id TEXT NOT NULL,
                 direction INTEGER NOT NULL CHECK (direction IN (-1, 1)),
                 created_at INTEGER NOT NULL DEFAULT (unixepoch()),
                 PRIMARY KEY (voter_pubkey, target_id)
             );
             CREATE TABLE IF NOT EXISTS social_follows (
                 follower_pubkey TEXT NOT NULL,
                 followed_pubkey TEXT NOT NULL,
                 created_at INTEGER NOT NULL DEFAULT (unixepoch()),
                 PRIMARY KEY (follower_pubkey, followed_pubkey)
             );
             CREATE TABLE IF NOT EXISTS social_community_members (
                 pubkey TEXT NOT NULL,
                 community_slug TEXT NOT NULL,
                 joined_at INTEGER NOT NULL DEFAULT (unixepoch()),
                 PRIMARY KEY (pubkey, community_slug)
             );
             CREATE TABLE IF NOT EXISTS social_messages (
                 id TEXT PRIMARY KEY,
                 sender_pubkey TEXT NOT NULL,
                 recipient_pubkey TEXT NOT NULL,
                 body TEXT NOT NULL,
                 read_at INTEGER,
                 created_at INTEGER NOT NULL DEFAULT (unixepoch())
             );
             CREATE TABLE IF NOT EXISTS social_moderation_reports (
                 id TEXT PRIMARY KEY,
                 reporter_pubkey TEXT NOT NULL,
                 target_id TEXT NOT NULL,
                 reason TEXT NOT NULL,
                 status TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'resolved', 'dismissed')),
                 created_at INTEGER NOT NULL DEFAULT (unixepoch()),
                 resolved_at INTEGER
             );
             CREATE TABLE IF NOT EXISTS social_bookmarks (
                 pubkey TEXT NOT NULL,
                 target_id TEXT NOT NULL,
                 created_at INTEGER NOT NULL DEFAULT (unixepoch()),
                 PRIMARY KEY (pubkey, target_id)
             );
             CREATE INDEX IF NOT EXISTS idx_posts_platform ON social_posts (platform, created_at DESC);
             CREATE INDEX IF NOT EXISTS idx_posts_community ON social_posts (community_slug, created_at DESC);
             CREATE INDEX IF NOT EXISTS idx_posts_author ON social_posts (author_pubkey, created_at DESC);
             CREATE INDEX IF NOT EXISTS idx_comments_post ON social_comments (post_id, created_at);
             CREATE INDEX IF NOT EXISTS idx_votes_target ON social_votes (target_id);
             CREATE INDEX IF NOT EXISTS idx_follows_followed ON social_follows (followed_pubkey);
             CREATE INDEX IF NOT EXISTS idx_follows_follower ON social_follows (follower_pubkey);
             CREATE INDEX IF NOT EXISTS idx_messages_recipient ON social_messages (recipient_pubkey, created_at DESC);
             CREATE INDEX IF NOT EXISTS idx_messages_sender ON social_messages (sender_pubkey, created_at DESC);",
        )
        .map_err(|e| format!("social schema init failed: {e}"))?;

        let version: u32 = conn
            .query_row(
                "SELECT value FROM social_meta WHERE key = 'schema_version'",
                [],
                |row| row.get::<_, String>(0),
            )
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        if version < CURRENT_VERSION {
            conn.execute(
                "INSERT OR REPLACE INTO social_meta (key, value) VALUES ('schema_version', ?1)",
                params![CURRENT_VERSION.to_string()],
            )
            .map_err(|e| format!("social version write failed: {e}"))?;
        }
        Ok(())
    }

    pub fn upsert_user(
        &self,
        pubkey: &str,
        display_name: &str,
        bio: &str,
        avatar_url: &str,
    ) -> Result<(), String> {
        self.lock()?
            .execute(
                "INSERT OR REPLACE INTO social_users (pubkey, display_name, bio, avatar_url) VALUES (?1, ?2, ?3, ?4)",
                params![pubkey, display_name, bio, avatar_url],
            )
            .map_err(|e| format!("upsert user failed: {e}"))?;
        Ok(())
    }

    pub fn get_user(&self, pubkey: &str) -> Result<Option<UserRow>, String> {
        self.lock()?
            .query_row(
                "SELECT pubkey, display_name, bio, avatar_url, created_at FROM social_users WHERE pubkey = ?1",
                params![pubkey],
                |row| {
                    Ok(UserRow {
                        pubkey: row.get(0)?,
                        display_name: row.get(1)?,
                        bio: row.get(2)?,
                        avatar_url: row.get(3)?,
                        created_at: row.get(4)?,
                    })
                },
            )
            .optional()
            .map_err(|e| format!("get user failed: {e}"))
    }

    pub fn insert_post(&self, post: &SocialPost) -> Result<(), String> {
        let media_json =
            serde_json::to_string(&post.media_urls).unwrap_or_else(|_| "[]".to_string());
        self.lock()?
            .execute(
                "INSERT OR REPLACE INTO social_posts (id, platform, author_pubkey, title, body, community_slug, parent_id, media_urls)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    post.id,
                    platform_label(post.platform),
                    post.author_pubkey,
                    post.title.as_deref().unwrap_or(""),
                    post.body,
                    post.community_slug.as_deref().unwrap_or(""),
                    post.parent_id.as_deref().unwrap_or(""),
                    media_json,
                ],
            )
            .map_err(|e| format!("insert post failed: {e}"))?;
        Ok(())
    }

    pub fn list_posts(
        &self,
        platform: SocialPlatform,
        community_slug: Option<&str>,
        author_pubkey: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<PostRow>, String> {
        let platform_label = platform_label(platform);
        let conn = self.lock()?;
        let mut stmt = if let Some(_slug) = community_slug {
            conn.prepare(
                "SELECT id, platform, author_pubkey, title, body, community_slug, vote_score, created_at FROM social_posts
                 WHERE platform = ?1 AND community_slug = ?2 ORDER BY created_at DESC LIMIT ?3 OFFSET ?4",
            )
            .map_err(|e| format!("post query failed: {e}"))?
        } else if let Some(_pubkey) = author_pubkey {
            conn.prepare(
                "SELECT id, platform, author_pubkey, title, body, community_slug, vote_score, created_at FROM social_posts
                 WHERE platform = ?1 AND author_pubkey = ?2 ORDER BY created_at DESC LIMIT ?3 OFFSET ?4",
            )
            .map_err(|e| format!("post query failed: {e}"))?
        } else {
            conn.prepare(
                "SELECT id, platform, author_pubkey, title, body, community_slug, vote_score, created_at FROM social_posts
                 WHERE platform = ?1 ORDER BY created_at DESC LIMIT ?2 OFFSET ?3",
            )
            .map_err(|e| format!("post query failed: {e}"))?
        };

        let rows = if let Some(slug) = community_slug {
            stmt.query_map(
                params![platform_label, slug, limit as i64, offset as i64],
                row_to_post,
            )
            .map_err(|e| format!("post query failed: {e}"))?
        } else if let Some(pubkey) = author_pubkey {
            stmt.query_map(
                params![platform_label, pubkey, limit as i64, offset as i64],
                row_to_post,
            )
            .map_err(|e| format!("post query failed: {e}"))?
        } else {
            stmt.query_map(
                params![platform_label, limit as i64, offset as i64],
                row_to_post,
            )
            .map_err(|e| format!("post query failed: {e}"))?
        };

        let mut out = Vec::with_capacity(limit);
        for row in rows {
            out.push(row.map_err(|e| format!("post row failed: {e}"))?);
        }
        Ok(out)
    }

    pub fn get_post(&self, id: &str) -> Result<Option<PostRow>, String> {
        self.lock()?
            .query_row(
                "SELECT id, platform, author_pubkey, title, body, community_slug, vote_score, created_at FROM social_posts WHERE id = ?1",
                params![id],
                row_to_post,
            )
            .optional()
            .map_err(|e| format!("get post failed: {e}"))
    }

    pub fn delete_post(&self, id: &str, author_pubkey: &str) -> Result<bool, String> {
        let n = self
            .lock()?
            .execute(
                "DELETE FROM social_posts WHERE id = ?1 AND author_pubkey = ?2",
                params![id, author_pubkey],
            )
            .map_err(|e| format!("delete post failed: {e}"))?;
        Ok(n > 0)
    }
    pub fn insert_comment(&self, comment: &SocialComment) -> Result<(), String> {
        let parent: Option<&str> = comment.parent_id.as_deref().filter(|s| !s.is_empty());
        self.lock()?
            .execute(
                "INSERT OR REPLACE INTO social_comments (id, platform, author_pubkey, post_id, parent_id, body)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    comment.id,
                    platform_label(comment.platform),
                    comment.author_pubkey,
                    comment.post_id,
                    parent,
                    comment.body,
                ],
            )
            .map_err(|e| format!("insert comment failed: {e}"))?;
        Ok(())
    }

    pub fn list_comments(&self, post_id: &str) -> Result<Vec<CommentRow>, String> {
        let conn = self.lock()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, platform, author_pubkey, post_id, parent_id, body, vote_score, created_at
                 FROM social_comments WHERE post_id = ?1 ORDER BY created_at",
            )
            .map_err(|e| format!("comment query failed: {e}"))?;
        let rows = stmt
            .query_map(params![post_id], |row| {
                Ok(CommentRow {
                    id: row.get(0)?,
                    platform: row.get(1)?,
                    author_pubkey: row.get(2)?,
                    post_id: row.get(3)?,
                    parent_id: row.get(4)?,
                    body: row.get(5)?,
                    vote_score: row.get(6)?,
                    created_at: row.get(7)?,
                })
            })
            .map_err(|e| format!("comment query failed: {e}"))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| format!("comment row failed: {e}"))?);
        }
        Ok(out)
    }

    pub fn vote(
        &self,
        voter_pubkey: &str,
        target_id: &str,
        direction: i32,
        target_table: &str,
    ) -> Result<(), String> {
        self.lock()?
            .execute(
                "INSERT OR REPLACE INTO social_votes (voter_pubkey, target_id, direction) VALUES (?1, ?2, ?3)",
                params![voter_pubkey, target_id, direction],
            )
            .map_err(|e| format!("vote failed: {e}"))?;
        let score_change = direction as i64;
        match target_table {
            "posts" => {
                self.lock()?
                    .execute(
                        "UPDATE social_posts SET vote_score = vote_score + ?1 WHERE id = ?2",
                        params![score_change, target_id],
                    )
                    .map_err(|e| format!("vote score update failed: {e}"))?;
            }
            "comments" => {
                self.lock()?
                    .execute(
                        "UPDATE social_comments SET vote_score = vote_score + ?1 WHERE id = ?2",
                        params![score_change, target_id],
                    )
                    .map_err(|e| format!("comment score update failed: {e}"))?;
            }
            _ => {}
        }
        Ok(())
    }

    pub fn follow(&self, follower_pubkey: &str, followed_pubkey: &str) -> Result<(), String> {
        self.lock()?
            .execute(
                "INSERT OR IGNORE INTO social_follows (follower_pubkey, followed_pubkey) VALUES (?1, ?2)",
                params![follower_pubkey, followed_pubkey],
            )
            .map_err(|e| format!("follow failed: {e}"))?;
        Ok(())
    }

    pub fn unfollow(&self, follower_pubkey: &str, followed_pubkey: &str) -> Result<(), String> {
        self.lock()?
            .execute(
                "DELETE FROM social_follows WHERE follower_pubkey = ?1 AND followed_pubkey = ?2",
                params![follower_pubkey, followed_pubkey],
            )
            .map_err(|e| format!("unfollow failed: {e}"))?;
        Ok(())
    }

    pub fn is_following(
        &self,
        follower_pubkey: &str,
        followed_pubkey: &str,
    ) -> Result<bool, String> {
        self.lock()?
            .query_row(
                "SELECT 1 FROM social_follows WHERE follower_pubkey = ?1 AND followed_pubkey = ?2",
                params![follower_pubkey, followed_pubkey],
                |_| Ok(()),
            )
            .optional()
            .map(|r| r.is_some())
            .map_err(|e| format!("follow check failed: {e}"))
    }

    pub fn join_community(&self, pubkey: &str, community_slug: &str) -> Result<(), String> {
        self.lock()?
            .execute(
                "INSERT OR IGNORE INTO social_community_members (pubkey, community_slug) VALUES (?1, ?2)",
                params![pubkey, community_slug],
            )
            .map_err(|e| format!("community join failed: {e}"))?;
        Ok(())
    }

    pub fn leave_community(&self, pubkey: &str, community_slug: &str) -> Result<(), String> {
        self.lock()?
            .execute(
                "DELETE FROM social_community_members WHERE pubkey = ?1 AND community_slug = ?2",
                params![pubkey, community_slug],
            )
            .map_err(|e| format!("community leave failed: {e}"))?;
        Ok(())
    }

    pub fn insert_message(
        &self,
        id: &str,
        sender_pubkey: &str,
        recipient_pubkey: &str,
        body: &str,
    ) -> Result<(), String> {
        self.lock()?
            .execute(
                "INSERT OR REPLACE INTO social_messages (id, sender_pubkey, recipient_pubkey, body)
                 VALUES (?1, ?2, ?3, ?4)",
                params![id, sender_pubkey, recipient_pubkey, body],
            )
            .map_err(|e| format!("insert message failed: {e}"))?;
        Ok(())
    }

    pub fn list_messages(&self, user_pubkey: &str) -> Result<Vec<MessageRow>, String> {
        let conn = self.lock()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, sender_pubkey, recipient_pubkey, body, read_at, created_at
                 FROM social_messages WHERE sender_pubkey = ?1 OR recipient_pubkey = ?1
                 ORDER BY created_at DESC LIMIT 500",
            )
            .map_err(|e| format!("messages query failed: {e}"))?;
        let rows = stmt
            .query_map(params![user_pubkey], |row| {
                Ok(MessageRow {
                    id: row.get(0)?,
                    sender_pubkey: row.get(1)?,
                    recipient_pubkey: row.get(2)?,
                    body: row.get(3)?,
                    read_at: row.get(4)?,
                    created_at: row.get(5)?,
                })
            })
            .map_err(|e| format!("messages query failed: {e}"))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| format!("message row failed: {e}"))?);
        }
        Ok(out)
    }

    pub fn submit_report(
        &self,
        id: &str,
        reporter_pubkey: &str,
        target_id: &str,
        reason: &str,
    ) -> Result<(), String> {
        self.lock()?
            .execute(
                "INSERT OR REPLACE INTO social_moderation_reports (id, reporter_pubkey, target_id, reason)
                 VALUES (?1, ?2, ?3, ?4)",
                params![id, reporter_pubkey, target_id, reason],
            )
            .map_err(|e| format!("report submit failed: {e}"))?;
        Ok(())
    }

    pub fn resolve_report(&self, id: &str, status: &str) -> Result<(), String> {
        self.lock()?
            .execute(
                "UPDATE social_moderation_reports SET status = ?1, resolved_at = unixepoch() WHERE id = ?2",
                params![status, id],
            )
            .map_err(|e| format!("report resolve failed: {e}"))?;
        Ok(())
    }

    pub fn add_bookmark(&self, pubkey: &str, target_id: &str) -> Result<(), String> {
        self.lock()?
            .execute(
                "INSERT OR IGNORE INTO social_bookmarks (pubkey, target_id) VALUES (?1, ?2)",
                params![pubkey, target_id],
            )
            .map_err(|e| format!("bookmark add failed: {e}"))?;
        Ok(())
    }

    pub fn remove_bookmark(&self, pubkey: &str, target_id: &str) -> Result<(), String> {
        self.lock()?
            .execute(
                "DELETE FROM social_bookmarks WHERE pubkey = ?1 AND target_id = ?2",
                params![pubkey, target_id],
            )
            .map_err(|e| format!("bookmark remove failed: {e}"))?;
        Ok(())
    }

    pub fn is_bookmarked(&self, pubkey: &str, target_id: &str) -> Result<bool, String> {
        self.lock()?
            .query_row(
                "SELECT 1 FROM social_bookmarks WHERE pubkey = ?1 AND target_id = ?2",
                params![pubkey, target_id],
                |_| Ok(()),
            )
            .optional()
            .map(|r| r.is_some())
            .map_err(|e| format!("bookmark check failed: {e}"))
    }

    pub fn list_bookmarks(&self, pubkey: &str) -> Result<Vec<String>, String> {
        let conn = self.lock()?;
        let mut stmt = conn
            .prepare(
                "SELECT target_id FROM social_bookmarks WHERE pubkey = ?1 ORDER BY created_at DESC",
            )
            .map_err(|e| format!("bookmarks query failed: {e}"))?;
        let rows = stmt
            .query_map(params![pubkey], |row| row.get::<_, String>(0))
            .map_err(|e| format!("bookmarks query failed: {e}"))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| format!("bookmark row failed: {e}"))?);
        }
        Ok(out)
    }

    pub fn list_open_reports(&self) -> Result<Vec<ReportRow>, String> {
        let conn = self.lock()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, reporter_pubkey, target_id, reason, status, created_at
                 FROM social_moderation_reports WHERE status = 'open' ORDER BY created_at",
            )
            .map_err(|e| format!("reports query failed: {e}"))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(ReportRow {
                    id: row.get(0)?,
                    reporter_pubkey: row.get(1)?,
                    target_id: row.get(2)?,
                    reason: row.get(3)?,
                    status: row.get(4)?,
                    created_at: row.get(5)?,
                })
            })
            .map_err(|e| format!("reports query failed: {e}"))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| format!("report row failed: {e}"))?);
        }
        Ok(out)
    }
}

fn platform_label(platform: SocialPlatform) -> &'static str {
    match platform {
        SocialPlatform::Tuitter => "Tuitter",
        SocialPlatform::Redit => "Redit",
    }
}

fn row_to_post(row: &rusqlite::Row<'_>) -> rusqlite::Result<PostRow> {
    Ok(PostRow {
        id: row.get(0)?,
        platform: row.get(1)?,
        author_pubkey: row.get(2)?,
        title: row.get(3)?,
        body: row.get(4)?,
        community_slug: row.get(5)?,
        vote_score: row.get(6)?,
        created_at: row.get(7)?,
    })
}

#[derive(Debug, Clone)]
pub struct UserRow {
    pub pubkey: String,
    pub display_name: String,
    pub bio: String,
    pub avatar_url: String,
    pub created_at: i64,
}

#[derive(Debug, Clone)]
pub struct PostRow {
    pub id: String,
    pub platform: String,
    pub author_pubkey: String,
    pub title: String,
    pub body: String,
    pub community_slug: String,
    pub vote_score: i64,
    pub created_at: i64,
}

#[derive(Debug, Clone)]
pub struct CommentRow {
    pub id: String,
    pub platform: String,
    pub author_pubkey: String,
    pub post_id: String,
    pub parent_id: Option<String>,
    pub body: String,
    pub vote_score: i64,
    pub created_at: i64,
}

#[derive(Debug, Clone)]
pub struct MessageRow {
    pub id: String,
    pub sender_pubkey: String,
    pub recipient_pubkey: String,
    pub body: String,
    pub read_at: Option<i64>,
    pub created_at: i64,
}

#[derive(Debug, Clone)]
pub struct ReportRow {
    pub id: String,
    pub reporter_pubkey: String,
    pub target_id: String,
    pub reason: String,
    pub status: String,
    pub created_at: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::Identity;

    fn identity() -> Identity {
        Identity::generate()
    }

    #[test]
    fn test_social_store_schema_init_and_upgrade() {
        let store = SocialStore::open_in_memory().expect("open");
        let conn = store.lock().unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(count >= 8, "at least 8 social tables expected");
    }

    #[test]
    fn test_upsert_and_get_user() {
        let store = SocialStore::open_in_memory().unwrap();
        let id = identity();
        let pubkey = id.pubkey_hex();
        store.upsert_user(&pubkey, "Alice", "hello", "").unwrap();
        let user = store.get_user(&pubkey).unwrap().expect("user exists");
        assert_eq!(user.display_name, "Alice");
    }

    #[test]
    fn test_post_insert_and_list() {
        let store = SocialStore::open_in_memory().unwrap();
        let id = identity();
        let pubkey = id.pubkey_hex();
        store.upsert_user(&pubkey, "Alice", "", "").unwrap();

        let post = SocialPost {
            id: "post-1".to_string(),
            platform: SocialPlatform::Tuitter,
            author_pubkey: pubkey.clone(),
            title: None,
            body: "test post".to_string(),
            community_slug: None,
            parent_id: None,
            media_urls: vec![],
        };
        store.insert_post(&post).unwrap();
        let posts = store
            .list_posts(SocialPlatform::Tuitter, None, None, 10, 0)
            .unwrap();
        assert_eq!(posts.len(), 1);
        assert_eq!(posts[0].body, "test post");
    }

    #[test]
    fn test_redit_post_with_community() {
        let store = SocialStore::open_in_memory().unwrap();
        let id = identity();
        let pubkey = id.pubkey_hex();
        store.upsert_user(&pubkey, "Bob", "", "").unwrap();

        let post = SocialPost {
            id: "post-r".to_string(),
            platform: SocialPlatform::Redit,
            author_pubkey: pubkey.clone(),
            title: Some("Hello".to_string()),
            body: "discussion body".to_string(),
            community_slug: Some("entraide".to_string()),
            parent_id: None,
            media_urls: vec![],
        };
        store.insert_post(&post).unwrap();
        let posts = store
            .list_posts(SocialPlatform::Redit, Some("entraide"), None, 10, 0)
            .unwrap();
        assert_eq!(posts.len(), 1);
        assert_eq!(posts[0].title, "Hello");
    }

    #[test]
    fn test_comments_and_votes() {
        let store = SocialStore::open_in_memory().unwrap();
        let id = identity();
        let pubkey = id.pubkey_hex();
        store.upsert_user(&pubkey, "voter", "", "").unwrap();

        let post = SocialPost {
            id: "p".to_string(),
            platform: SocialPlatform::Tuitter,
            author_pubkey: pubkey.clone(),
            title: None,
            body: "post".to_string(),
            community_slug: None,
            parent_id: None,
            media_urls: vec![],
        };
        store.insert_post(&post).unwrap();

        let comment = SocialComment {
            id: "c".to_string(),
            platform: SocialPlatform::Tuitter,
            author_pubkey: pubkey.clone(),
            post_id: "p".to_string(),
            parent_id: None,
            body: "comment".to_string(),
        };
        store.insert_comment(&comment).unwrap();
        let comments = store.list_comments("p").unwrap();
        assert_eq!(comments.len(), 1);

        store.vote(&pubkey, "c", 1, "comments").unwrap();
    }

    #[test]
    fn test_follow_unfollow_and_membership() {
        let store = SocialStore::open_in_memory().unwrap();
        let alice = identity();
        let bob = identity();

        store
            .follow(&alice.pubkey_hex(), &bob.pubkey_hex())
            .unwrap();
        assert!(store
            .is_following(&alice.pubkey_hex(), &bob.pubkey_hex())
            .unwrap());

        store
            .unfollow(&alice.pubkey_hex(), &bob.pubkey_hex())
            .unwrap();
        assert!(!store
            .is_following(&alice.pubkey_hex(), &bob.pubkey_hex())
            .unwrap());

        store.join_community(&bob.pubkey_hex(), "entraide").unwrap();
        store
            .leave_community(&bob.pubkey_hex(), "entraide")
            .unwrap();
    }

    #[test]
    fn test_messages_roundtrip() {
        let store = SocialStore::open_in_memory().unwrap();
        let alice = identity();
        let bob = identity();

        store
            .insert_message("msg-1", &alice.pubkey_hex(), &bob.pubkey_hex(), "salut")
            .unwrap();
        let alice_msgs = store.list_messages(&alice.pubkey_hex()).unwrap();
        let bob_msgs = store.list_messages(&bob.pubkey_hex()).unwrap();
        assert_eq!(alice_msgs.len(), 1);
        assert_eq!(bob_msgs.len(), 1);
        assert_eq!(alice_msgs[0].body, "salut");
    }

    #[test]
    fn test_moderation_report_flow() {
        let store = SocialStore::open_in_memory().unwrap();
        store
            .submit_report("rep-1", "reporter", "bad-post", "spam")
            .unwrap();
        let open = store.list_open_reports().unwrap();
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].status, "open");

        store.resolve_report("rep-1", "resolved").unwrap();
        let open2 = store.list_open_reports().unwrap();
        assert!(open2.is_empty());
    }

    #[test]
    fn test_delete_post() {
        let store = SocialStore::open_in_memory().unwrap();
        let id = identity();
        let pubkey = id.pubkey_hex();
        store.upsert_user(&pubkey, "a", "", "").unwrap();

        let post = SocialPost {
            id: "del-1".to_string(),
            platform: SocialPlatform::Tuitter,
            author_pubkey: pubkey.clone(),
            title: None,
            body: "to delete".to_string(),
            community_slug: None,
            parent_id: None,
            media_urls: vec![],
        };
        store.insert_post(&post).unwrap();
        assert!(store.delete_post("del-1", &pubkey).unwrap());
        assert!(store.get_post("del-1").unwrap().is_none());

        // Wrong author can't delete
        let alien = identity();
        store.insert_post(&post).unwrap();
        assert!(!store.delete_post("del-1", &alien.pubkey_hex()).unwrap());
        assert!(store.get_post("del-1").unwrap().is_some());
    }
}
