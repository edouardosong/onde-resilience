/// Persistance SQLite du graphe social partagé Tuitter/Redit.
///
/// Chaque nœud stocke localement ses données sociales ; le gossip ONDE assure
/// la réplication entre nœuds (les événements signés sont les sources de
/// vérité). Ce store est un cache matérialisé, pas une autorité.
use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension};

use crate::social::{SocialComment, SocialPlatform, SocialPost};

// v2 : table social_orphan_comments — buffer des commentaires arrivés AVANT
// leur post (banal en DTN/gossip) au lieu d'un échec FOREIGN KEY.
const CURRENT_VERSION: u32 = 2;

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
             CREATE INDEX IF NOT EXISTS idx_messages_sender ON social_messages (sender_pubkey, created_at DESC);
             -- Buffer anti-orphelins (v2) : commentaires reçus avant leur post.
             -- SANS clés étrangères volontairement : le store est un cache
             -- matérialisé, un cache-miss ne doit jamais être une erreur.
             CREATE TABLE IF NOT EXISTS social_orphan_comments (
                 id TEXT PRIMARY KEY,
                 platform TEXT NOT NULL CHECK (platform IN ('Tuitter', 'Redit')),
                 author_pubkey TEXT NOT NULL,
                 post_id TEXT NOT NULL,
                 parent_id TEXT,
                 body TEXT NOT NULL,
                 created_at INTEGER NOT NULL DEFAULT (unixepoch())
             );
             CREATE INDEX IF NOT EXISTS idx_orphan_comments_post ON social_orphan_comments (post_id, created_at);",
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

    /// Crée l'utilisateur S'IL N'EXISTE PAS — n'écrase JAMAIS un profil
    /// existant (nom d'affichage, bio, avatar). À utiliser sur le chemin de
    /// réception et toute création implicite d'auteur ;
    /// [`Self::upsert_user`] reste réservé à la mise à jour explicite d'un
    /// profil par son propriétaire.
    pub fn ensure_user(&self, pubkey: &str) -> Result<(), String> {
        self.lock()?
            .execute(
                "INSERT OR IGNORE INTO social_users (pubkey, display_name, bio, avatar_url) VALUES (?1, '', '', '')",
                params![pubkey],
            )
            .map_err(|e| format!("ensure user failed: {e}"))?;
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

    /// Insère un post puis REJOUE les commentaires bufferisés qui le
    /// concernent (cache-miss → rejeu, voir [`Self::insert_comment`]).
    pub fn insert_post(&self, post: &SocialPost) -> Result<(), String> {
        let media_json =
            serde_json::to_string(&post.media_urls).unwrap_or_else(|_| "[]".to_string());
        {
            let conn = self.lock()?;
            conn.execute(
                // NB : la colonne parent_id (héritage Fusion) n'est plus
                // alimentée — l'imbrication appartient aux commentaires (I1).
                "INSERT OR REPLACE INTO social_posts (id, platform, author_pubkey, title, body, community_slug, parent_id, media_urls)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, ?7)",
                params![
                    post.id,
                    platform_label(post.platform),
                    post.author_pubkey,
                    post.title.as_deref().unwrap_or(""),
                    post.body,
                    post.community_slug.as_deref().unwrap_or(""),
                    media_json,
                ],
            )
            .map_err(|e| format!("insert post failed: {e}"))?;
        }
        // Le verrou est relâché : la promotion reprend le mutex.
        self.promote_orphan_comments(&post.id)
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
    /// Insère un commentaire dans le cache. Le store est un **cache
    /// matérialisé**, pas une autorité : un commentaire arrivé AVANT son post
    /// (banal en DTN/gossip) ou avant son commentaire parent n'est PAS une
    /// erreur — il est bufferisé dans `social_orphan_comments` puis rejoué
    /// quand le post arrive ([`Self::insert_post`]).
    ///
    /// Retourne `Ok(true)` si stocké dans `social_comments`, `Ok(false)` si
    /// bufferisé en attente de son parent.
    pub fn insert_comment(&self, comment: &SocialComment) -> Result<bool, String> {
        // Un parent encore présent UNIQUEMENT dans le buffer n'est pas prêt :
        // la réponse est bufferisée à son tour et sera promue par la boucle
        // de rejeu (voir [`Self::promote_orphan_comments`]).
        let stored = {
            let conn = self.lock()?;
            let parent: Option<&str> = comment.parent_id.as_deref().filter(|s| !s.is_empty());
            let post_ready = conn
                .query_row(
                    "SELECT 1 FROM social_posts WHERE id = ?1",
                    params![comment.post_id],
                    |_| Ok(()),
                )
                .optional()
                .map_err(|e| format!("post lookup failed: {e}"))?
                .is_some();
            let parent_ready = parent.map_or(Ok::<_, String>(true), |p| {
                Ok(conn
                    .query_row(
                        "SELECT 1 FROM social_comments WHERE id = ?1",
                        params![p],
                        |_| Ok(()),
                    )
                    .optional()
                    .map_err(|e| format!("parent lookup failed: {e}"))?
                    .is_some())
            })?;

            if post_ready && parent_ready {
                ensure_user_tx(&conn, &comment.author_pubkey)?;
                conn.execute(
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
                // S'il était déjà bufferisé (doublon de gossip), nettoie le buffer.
                conn.execute(
                    "DELETE FROM social_orphan_comments WHERE id = ?1",
                    params![comment.id],
                )
                .map_err(|e| format!("orphan cleanup failed: {e}"))?;
                Ok::<bool, String>(true)
            } else {
                // Buffer anti-orphelins — borné pour ne jamais croître sans limite.
                conn.execute(
                "INSERT OR REPLACE INTO social_orphan_comments (id, platform, author_pubkey, post_id, parent_id, body)
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
            .map_err(|e| format!("orphan buffer write failed: {e}"))?;
                conn.execute(
                    "DELETE FROM social_orphan_comments WHERE id IN (
                     SELECT id FROM social_orphan_comments
                     ORDER BY created_at DESC, id DESC LIMIT -1 OFFSET ?1
                 )",
                    params![MAX_ORPHAN_COMMENTS as i64],
                )
                .map_err(|e| format!("orphan buffer cap failed: {e}"))?;
                Ok(false)
            }
        }?;
        // Verrou relâché : si ce commentaire vient d'être stocké, tente de
        // promouvoir d'éventuelles réponses qui l'attendaient (cas B).
        if stored {
            self.promote_orphan_comments(&comment.post_id)?;
        }
        Ok(stored)
    }

    /// Nombre de commentaires actuellement bufferisés (observabilité/tests).
    pub fn orphan_comment_count(&self) -> Result<usize, String> {
        let conn = self.lock()?;
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM social_orphan_comments", [], |r| {
                r.get(0)
            })
            .map_err(|e| format!("orphan count failed: {e}"))?;
        Ok(n as usize)
    }

    /// Rejoue les commentaires bufferisés d'un post fraîchement arrivé.
    /// Boucle tant que des promotements progressent : couvre les chaînes de
    /// réponses imbriquées arrivées dans un ordre quelconque. Une réponse dont
    /// le commentaire parent n'existe toujours pas reste en buffer.
    fn promote_orphan_comments(&self, post_id: &str) -> Result<(), String> {
        let conn = self.lock()?;
        loop {
            // NB : le corps est copié par l'INSERT…SELECT depuis la table
            // buffer — pas besoin de le charger en mémoire ici.
            let pending: Vec<(String, String, Option<String>)> = {
                let mut stmt = conn
                    .prepare(
                        "SELECT id, author_pubkey, parent_id FROM social_orphan_comments
                         WHERE post_id = ?1 ORDER BY created_at, id LIMIT 256",
                    )
                    .map_err(|e| format!("orphan scan failed: {e}"))?;
                let rows = stmt
                    .query_map(params![post_id], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, Option<String>>(2)?,
                        ))
                    })
                    .map_err(|e| format!("orphan scan failed: {e}"))?;
                rows.collect::<Result<Vec<_>, _>>()
                    .map_err(|e| format!("orphan row failed: {e}"))?
            };
            if pending.is_empty() {
                return Ok(());
            }
            let mut progressed = false;
            for (id, author, parent_id) in pending {
                let parent_ok = parent_id.as_deref().map_or(Ok::<_, String>(true), |p| {
                    Ok(conn
                        .query_row(
                            "SELECT 1 FROM social_comments WHERE id = ?1",
                            params![p],
                            |_| Ok(()),
                        )
                        .optional()
                        .map_err(|e| format!("parent lookup failed: {e}"))?
                        .is_some())
                })?;
                if !parent_ok {
                    continue;
                }
                ensure_user_tx(&conn, &author)?;
                conn.execute(
                    "INSERT OR IGNORE INTO social_comments (id, platform, author_pubkey, post_id, parent_id, body)
                     SELECT id, platform, author_pubkey, post_id, parent_id, body
                     FROM social_orphan_comments WHERE id = ?1",
                    params![id],
                )
                .map_err(|e| format!("orphan promote failed: {e}"))?;
                conn.execute(
                    "DELETE FROM social_orphan_comments WHERE id = ?1",
                    params![id],
                )
                .map_err(|e| format!("orphan cleanup failed: {e}"))?;
                progressed = true;
            }
            if !progressed {
                return Ok(());
            }
        }
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

    /// Enregistre (ou met à jour) LE vote unique d'un voteur sur une cible et
    /// applique au score le **delta** par rapport à son vote précédent.
    ///
    /// Un même voteur ne pèse qu'une fois : N re-votes identiques déplacent le
    /// score d'au plus ±1 ; passer de +1 à -1 l'applique en un seul delta −2.
    /// Retourne le delta effectivement appliqué (0 si inchangé).
    pub fn vote(
        &self,
        voter_pubkey: &str,
        target_id: &str,
        direction: i32,
        target_table: &str,
    ) -> Result<i64, String> {
        let conn = self.lock()?;
        let previous: Option<i32> = conn
            .query_row(
                "SELECT direction FROM social_votes WHERE voter_pubkey = ?1 AND target_id = ?2",
                params![voter_pubkey, target_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("vote lookup failed: {e}"))?;
        let delta = i64::from(direction) - i64::from(previous.unwrap_or(0));
        conn.execute(
            "INSERT OR REPLACE INTO social_votes (voter_pubkey, target_id, direction) VALUES (?1, ?2, ?3)",
            params![voter_pubkey, target_id, direction],
        )
        .map_err(|e| format!("vote failed: {e}"))?;
        if delta != 0 {
            let table = match target_table {
                "posts" => "social_posts",
                "comments" => "social_comments",
                _ => return Ok(0),
            };
            // Identifiants issus d'une constante interne — pas d'injection.
            conn.execute(
                &format!("UPDATE {table} SET vote_score = vote_score + ?1 WHERE id = ?2"),
                params![delta, target_id],
            )
            .map_err(|e| format!("vote score update failed: {e}"))?;
        }
        Ok(delta)
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

/// Plafond global du buffer anti-orphelins (bornage mémoire simple).
const MAX_ORPHAN_COMMENTS: usize = 1024;

/// Assure la présence de l'auteur dans social_users DANS une transaction
/// existante (INSERT OR IGNORE — n'écrase jamais un profil).
fn ensure_user_tx(conn: &Connection, pubkey: &str) -> Result<(), String> {
    conn.execute(
        "INSERT OR IGNORE INTO social_users (pubkey, display_name, bio, avatar_url) VALUES (?1, '', '', '')",
        params![pubkey],
    )
    .map_err(|e| format!("ensure user failed: {e}"))?;
    Ok(())
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

    // ── T13-checker : régressions H1 / M2 / L1 ──────────────────────────

    #[test]
    fn test_orphan_comment_buffered_then_replayed_when_post_arrives() {
        // H1 : un commentaire arrivé AVANT son post (banal en DTN/gossip)
        // est bufferisé (Ok(false)), JAMAIS une erreur ; le post le rejoue.
        let store = SocialStore::open_in_memory().unwrap();
        let author = identity().pubkey_hex();

        let comment = SocialComment {
            id: "orphan-1".to_string(),
            platform: SocialPlatform::Tuitter,
            author_pubkey: author.clone(),
            post_id: "future-post".to_string(),
            parent_id: None,
            body: "réponse pressée".to_string(),
        };
        let buffered = store.insert_comment(&comment).unwrap();
        assert!(
            !buffered,
            "comment before post must be buffered, not an error"
        );
        assert_eq!(store.orphan_comment_count().unwrap(), 1);
        assert!(store.list_comments("future-post").unwrap().is_empty());

        // Le post arrive → le commentaire bufferisé est promu.
        let post = SocialPost {
            id: "future-post".to_string(),
            platform: SocialPlatform::Tuitter,
            author_pubkey: "ab".repeat(32),
            title: None,
            body: "le post tant attendu".to_string(),
            community_slug: None,
            media_urls: vec![],
        };
        store.ensure_user(&post.author_pubkey).unwrap();
        store.insert_post(&post).unwrap();

        let comments = store.list_comments("future-post").unwrap();
        assert_eq!(comments.len(), 1, "buffered comment must be replayed");
        assert_eq!(comments[0].id, "orphan-1");
        assert_eq!(comments[0].body, "réponse pressée");
        assert_eq!(store.orphan_comment_count().unwrap(), 0);
    }

    #[test]
    fn test_nested_orphan_chain_replays_in_any_order() {
        // Réponse imbriquée dont le parent est lui-même orphelin : les deux
        // finissent par atterrir quand le post + le parent arrivent.
        let store = SocialStore::open_in_memory().unwrap();
        let author = identity().pubkey_hex();

        let root = SocialComment {
            id: "c-root".to_string(),
            platform: SocialPlatform::Redit,
            author_pubkey: author.clone(),
            post_id: "p-chain".to_string(),
            parent_id: None,
            body: "racine".to_string(),
        };
        let reply = SocialComment {
            id: "c-reply".to_string(),
            platform: SocialPlatform::Redit,
            author_pubkey: author.clone(),
            post_id: "p-chain".to_string(),
            parent_id: Some("c-root".to_string()),
            body: "réponse".to_string(),
        };
        // La réponse arrive EN PREMIER (parent inconnu) → buffer.
        assert!(!store.insert_comment(&reply).unwrap());
        // La racine arrive avant son post → buffer aussi.
        assert!(!store.insert_comment(&root).unwrap());
        assert_eq!(store.orphan_comment_count().unwrap(), 2);

        let post = SocialPost {
            id: "p-chain".to_string(),
            platform: SocialPlatform::Redit,
            author_pubkey: "cd".repeat(32),
            title: Some("Chaîne".to_string()),
            body: "corps".to_string(),
            community_slug: Some("entraide".to_string()),
            media_urls: vec![],
        };
        store.ensure_user(&post.author_pubkey).unwrap();
        store.insert_post(&post).unwrap();

        let comments = store.list_comments("p-chain").unwrap();
        assert_eq!(comments.len(), 2, "both chain comments must be promoted");
        assert_eq!(store.orphan_comment_count().unwrap(), 0);
    }

    #[test]
    fn test_vote_delta_not_cumulative_for_repeat_votes() {
        // M2 : N re-votes du même voteur déplacent le score d'au plus ±1 ;
        // un changement de sens applique le delta exact (pas de cumul).
        let store = SocialStore::open_in_memory().unwrap();
        let voter = identity().pubkey_hex();
        let post = SocialPost {
            id: "p-vote".to_string(),
            platform: SocialPlatform::Tuitter,
            author_pubkey: "ee".repeat(32),
            title: None,
            body: "cible de votes".to_string(),
            community_slug: None,
            media_urls: vec![],
        };
        store.ensure_user(&post.author_pubkey).unwrap();
        store.insert_post(&post).unwrap();
        let score = || store.get_post("p-vote").unwrap().unwrap().vote_score;

        // Cinq re-votes identiques → +1 total seulement.
        for _ in 0..5 {
            let delta = store.vote(&voter, "p-vote", 1, "posts").unwrap();
            assert!(delta == 0 || delta == 1);
        }
        assert_eq!(score(), 1, "5 identical votes must count as ONE");

        // Passage +1 → -1 : delta -2 appliqué une seule fois.
        let delta = store.vote(&voter, "p-vote", -1, "posts").unwrap();
        assert_eq!(delta, -2);
        assert_eq!(score(), -1);

        // Re-vote identique (-1) : plus aucun effet.
        let delta = store.vote(&voter, "p-vote", -1, "posts").unwrap();
        assert_eq!(delta, 0);
        assert_eq!(score(), -1);
    }

    #[test]
    fn test_ensure_user_never_overwrites_existing_profile() {
        // L1 : la création implicite d'auteur ne doit PAS écraser un profil.
        let store = SocialStore::open_in_memory().unwrap();
        let pubkey = identity().pubkey_hex();
        store
            .upsert_user(&pubkey, "Alice", "bio d'Alice", "http://a")
            .unwrap();

        // Arrivée d'un événement signé par Alice sur un autre nœud…
        store.ensure_user(&pubkey).unwrap();
        let user = store.get_user(&pubkey).unwrap().expect("user exists");
        assert_eq!(user.display_name, "Alice");
        assert_eq!(user.bio, "bio d'Alice");
        assert_eq!(user.avatar_url, "http://a");
    }
}
