/// Persistance SQLite — résilience aux crashs (Audit #14, plan P0)
///
/// Le magasin hiérarchique en mémoire (`TieredMessageStore`) protège contre
/// le volume mais pas contre la perte de données au redémarrage. Ce module
/// ajoute un backing store SQLite (via `rusqlite`, SQLite compilé en
/// `bundled` — aucune dépendance système) qui conserve les messages reçus /
/// publiés pour qu'un nœud puisse reprendre là où il s'était arrêté.
///
/// Table `messages` :
/// ```sql
/// CREATE TABLE IF NOT EXISTS messages (
///     id            TEXT PRIMARY KEY,
///     tier          TEXT NOT NULL,          -- critical|important|normal|low
///     created_at    INTEGER NOT NULL,       -- unix secs
///     payload       BLOB NOT NULL,          -- payload compressé (Deflate)
///     original_size INTEGER NOT NULL,       -- taille brute avant compression
///     geohash       TEXT NOT NULL
/// );
/// ```
///
/// Table `meta` (clé/valeur simple — Audit B4) :
/// ```sql
/// CREATE TABLE IF NOT EXISTS meta (
///     key   TEXT PRIMARY KEY,
///     value TEXT NOT NULL
/// );
/// ```
/// Elle stocke des métadonnées de nœud comme la seed Ed25519
/// (`identity_seed`, hex) pour restaurer une identité stable au redémarrage.
use std::path::Path;
use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension};

use super::{MessageTier, TieredMessage};

/// Store SQLite pour les messages hiérarchiques.
///
/// La connexion est encapsulée dans un `std::sync::Mutex` : `rusqlite::Connection`
/// est `Send` mais pas `Sync` (RefCell interne), or le nœud est partagé entre
/// threads (Tauri `State`, `tokio::sync::Mutex`). Le mutex rend `SqliteStore`
/// `Send + Sync` sans coût perceptible à cette échelle.
pub struct SqliteStore {
    conn: Mutex<Connection>,
}

impl SqliteStore {
    /// Ouvrir (ou créer) la base SQLite au chemin `path`.
    ///
    /// Les erreurs d'E/S (disque plein, permissions…) remontent à l'appelant
    /// qui peut choisir de continuer en mémoire seule.
    pub fn open(path: &str) -> Result<Self, String> {
        let conn = Connection::open(path).map_err(|e| format!("SQLite open failed: {e}"))?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             CREATE TABLE IF NOT EXISTS messages (
                 id            TEXT PRIMARY KEY,
                 tier          TEXT NOT NULL,
                 created_at    INTEGER NOT NULL,
                 payload       BLOB NOT NULL,
                 original_size INTEGER NOT NULL,
                 geohash       TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS meta (
                 key   TEXT PRIMARY KEY,
                 value TEXT NOT NULL
             );",
        )
        .map_err(|e| format!("SQLite schema init failed: {e}"))?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Ouvrir une base en mémoire (tests).
    pub fn open_in_memory() -> Result<Self, String> {
        Self::open(":memory:")
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>, String> {
        self.conn
            .lock()
            .map_err(|_| "SQLite mutex poisoned".to_string())
    }

    /// Insérer ou remplacer un message.
    pub fn store(&self, msg: &TieredMessage) -> Result<(), String> {
        self.lock()?
            .execute(
                "INSERT OR REPLACE INTO messages
                    (id, tier, created_at, payload, original_size, geohash)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    msg.id,
                    msg.tier.label(),
                    msg.created_at as i64,
                    msg.payload,
                    msg.original_size as i64,
                    msg.geohash,
                ],
            )
            .map_err(|e| format!("SQLite store failed: {e}"))?;
        Ok(())
    }

    /// Récupérer un message par son id.
    pub fn get(&self, id: &str) -> Result<Option<TieredMessage>, String> {
        self.lock()?
            .query_row(
                "SELECT id, tier, created_at, payload, original_size, geohash
                 FROM messages WHERE id = ?1",
                params![id],
                |row| {
                    let tier_label: String = row.get(1)?;
                    let tier = parse_tier(&tier_label);
                    Ok(TieredMessage {
                        id: row.get(0)?,
                        tier,
                        created_at: row.get::<_, i64>(2)? as u64,
                        payload: row.get(3)?,
                        original_size: row.get::<_, i64>(4)? as usize,
                        geohash: row.get(5)?,
                    })
                },
            )
            .optional()
            .map_err(|e| format!("SQLite get failed: {e}"))
    }

    /// Charger tous les messages persistés.
    pub fn load_all(&self) -> Result<Vec<TieredMessage>, String> {
        let conn = self.lock()?;
        let mut stmt = conn
            .prepare("SELECT id, tier, created_at, payload, original_size, geohash FROM messages")
            .map_err(|e| format!("SQLite prepare failed: {e}"))?;
        let rows = stmt
            .query_map([], |row| {
                let tier_label: String = row.get(1)?;
                Ok(TieredMessage {
                    id: row.get(0)?,
                    tier: parse_tier(&tier_label),
                    created_at: row.get::<_, i64>(2)? as u64,
                    payload: row.get(3)?,
                    original_size: row.get::<_, i64>(4)? as usize,
                    geohash: row.get(5)?,
                })
            })
            .map_err(|e| format!("SQLite query failed: {e}"))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| format!("SQLite row failed: {e}"))?);
        }
        Ok(out)
    }

    /// Supprimer les messages expirés selon leur tier. Retourne le nombre de
    /// lignes purgées.
    pub fn sweep_expired(&self, now: u64) -> Result<usize, String> {
        // On purge par tier avec sa rétention propre : un message Critical est
        // conservé 7 jours, un Low 1 heure, etc.
        let conn = self.lock()?;
        let mut purged = 0usize;
        for tier in [
            MessageTier::Critical,
            MessageTier::Important,
            MessageTier::Normal,
            MessageTier::Low,
        ] {
            let cutoff = now.saturating_sub(tier.retention_secs()) as i64;
            let n = conn
                .execute(
                    "DELETE FROM messages WHERE tier = ?1 AND created_at < ?2",
                    params![tier.label(), cutoff],
                )
                .map_err(|e| format!("SQLite sweep failed: {e}"))?;
            purged += n;
        }
        Ok(purged)
    }

    /// Nombre de messages persistés.
    pub fn count(&self) -> Result<usize, String> {
        self.lock()?
            .query_row("SELECT COUNT(*) FROM messages", [], |row| {
                row.get::<_, i64>(0)
            })
            .map(|n| n as usize)
            .map_err(|e| format!("SQLite count failed: {e}"))
    }

    /// Écrire une métadonnée de nœud (table key/value `meta`, Audit B4).
    ///
    /// Utilisée pour persister la seed Ed25519 (`identity_seed`) afin que
    /// l'identité du nœud survive aux redémarrages. La valeur est stockée en
    /// clair dans la base locale — au même niveau de confiance que le fichier
    /// de base lui-même.
    pub fn set_meta(&self, key: &str, value: &str) -> Result<(), String> {
        self.lock()?
            .execute(
                "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)",
                params![key, value],
            )
            .map_err(|e| format!("SQLite set_meta failed: {e}"))?;
        Ok(())
    }

    /// Lire une métadonnée de nœud. Retourne `None` si la clé n'existe pas.
    pub fn get_meta(&self, key: &str) -> Result<Option<String>, String> {
        self.lock()?
            .query_row(
                "SELECT value FROM meta WHERE key = ?1",
                params![key],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|e| format!("SQLite get_meta failed: {e}"))
    }
}

/// Convertir un label de tier en `MessageTier` (fallback : Normal).
fn parse_tier(label: &str) -> MessageTier {
    match label {
        "critical" => MessageTier::Critical,
        "important" => MessageTier::Important,
        "low" => MessageTier::Low,
        _ => MessageTier::Normal,
    }
}

/// Chemin par défaut de la base de données (par profil).
pub fn default_db_path(data_dir: &Path, node_type_label: &str) -> String {
    data_dir
        .join(format!("onde-{node_type_label}.sqlite3"))
        .to_string_lossy()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(id: &str, tier: MessageTier, created_at: u64) -> TieredMessage {
        TieredMessage {
            id: id.to_string(),
            tier,
            created_at,
            payload: vec![1, 2, 3, 4],
            original_size: 100,
            geohash: "u09tunq".to_string(),
        }
    }

    #[test]
    fn test_open_schema_and_store_get() {
        let store = SqliteStore::open_in_memory().unwrap();
        let msg = sample("evt-1", MessageTier::Critical, 1_800_000_000);
        store.store(&msg).unwrap();

        let got = store.get("evt-1").unwrap().expect("message exists");
        assert_eq!(got.id, msg.id);
        assert_eq!(got.tier, MessageTier::Critical);
        assert_eq!(got.payload, msg.payload);

        // Idempotent : re-store avec le même id remplace (INSERT OR REPLACE)
        store
            .store(&sample("evt-1", MessageTier::Normal, 1_800_000_000))
            .unwrap();
        let got2 = store.get("evt-1").unwrap().unwrap();
        assert_eq!(got2.tier, MessageTier::Normal);
        assert_eq!(store.count().unwrap(), 1);
    }

    #[test]
    fn test_load_all_roundtrip() {
        let store = SqliteStore::open_in_memory().unwrap();
        store
            .store(&sample("a", MessageTier::Critical, 100))
            .unwrap();
        store.store(&sample("b", MessageTier::Low, 200)).unwrap();
        store
            .store(&sample("c", MessageTier::Important, 300))
            .unwrap();

        let all = store.load_all().unwrap();
        assert_eq!(all.len(), 3);
        let tiers: Vec<_> = all.iter().map(|m| m.tier).collect();
        assert!(tiers.contains(&MessageTier::Critical));
        assert!(tiers.contains(&MessageTier::Low));
        assert!(tiers.contains(&MessageTier::Important));
    }

    #[test]
    fn test_sweep_expired() {
        let store = SqliteStore::open_in_memory().unwrap();
        let now = 2_000_000_000u64;
        store
            .store(&sample("crit", MessageTier::Critical, now - 2 * 24 * 3600))
            .unwrap();
        store
            .store(&sample("norm", MessageTier::Normal, now - 7 * 3600))
            .unwrap();
        store
            .store(&sample("low", MessageTier::Low, now - 7200))
            .unwrap();

        let purged = store.sweep_expired(now).unwrap();
        assert_eq!(purged, 2, "Normal (7h > 6h) and Low (2h > 1h) expire");
        assert_eq!(store.count().unwrap(), 1);
        assert!(store.get("crit").unwrap().is_some());
        assert!(store.get("norm").unwrap().is_none());
        assert!(store.get("low").unwrap().is_none());
    }

    #[test]
    fn test_default_db_path() {
        let p = default_db_path(Path::new("/data/onde"), "mobile");
        assert_eq!(p, "/data/onde/onde-mobile.sqlite3");
    }

    #[test]
    fn test_file_backed_persistence_across_reopen() {
        let dir = std::env::temp_dir().join(format!("onde-sqlite-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.sqlite3");
        let path_str = path.to_string_lossy().to_string();

        // Écriture dans une première session
        {
            let store = SqliteStore::open(&path_str).unwrap();
            store
                .store(&sample("persist-1", MessageTier::Critical, 1_800_000_000))
                .unwrap();
        }

        // Réouverture : le message est toujours là (résilience aux crashs)
        {
            let store = SqliteStore::open(&path_str).unwrap();
            assert_eq!(store.count().unwrap(), 1);
            assert!(store.get("persist-1").unwrap().is_some());
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_meta_roundtrip() {
        let store = SqliteStore::open_in_memory().unwrap();
        // Absent au départ
        assert_eq!(store.get_meta("identity_seed").unwrap(), None);

        // Écriture puis lecture
        store.set_meta("identity_seed", &"ab".repeat(32)).unwrap();
        assert_eq!(
            store.get_meta("identity_seed").unwrap(),
            Some("ab".repeat(32))
        );

        // INSERT OR REPLACE : l'écrasement remplace la valeur
        store.set_meta("identity_seed", &"cd".repeat(32)).unwrap();
        assert_eq!(
            store.get_meta("identity_seed").unwrap(),
            Some("cd".repeat(32))
        );

        // Une autre clé est indépendante
        assert_eq!(store.get_meta("autre").unwrap(), None);
        // La table meta ne touche pas aux messages
        assert_eq!(store.count().unwrap(), 0);
    }

    #[test]
    fn test_meta_persisted_across_reopen() {
        let dir = std::env::temp_dir().join(format!("onde-sqlite-meta-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("meta.sqlite3");
        let path_str = path.to_string_lossy().to_string();

        {
            let store = SqliteStore::open(&path_str).unwrap();
            store.set_meta("identity_seed", &"ef".repeat(32)).unwrap();
        }

        // Réouverture : la métadonnée est toujours là
        {
            let store = SqliteStore::open(&path_str).unwrap();
            assert_eq!(
                store.get_meta("identity_seed").unwrap(),
                Some("ef".repeat(32))
            );
        }

        let _ = std::fs::remove_dir_all(&dir);
    }
}
