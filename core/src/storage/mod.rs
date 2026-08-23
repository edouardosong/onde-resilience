use serde::{Deserialize, Serialize};
/// Offline Storage — ZIM reader, MBTiles renderer, IPFS seeder
///
/// Honest-storage principle (prototype): a missing file fails loudly with an
/// `Err`. Demo data is only ever loaded through EXPLICIT methods
/// (`load_demo` / `register_demo_seeds`), never silently by default.
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use base64::Engine as _;

/// Persistance SQLite (Audit #14 — résilience aux crashs)
pub mod persistence;

/*
 * ZIM Archive Reader — Wikipedia Offline
 *
 * Reads .zim files (openZIM format) for offline encyclopedia access
 */

pub struct ZimReader {
    archive_path: Option<PathBuf>,
    article_cache: HashMap<String, Vec<u8>>,
    total_articles: u64,
}

impl ZimReader {
    pub fn new() -> Self {
        Self {
            archive_path: None,
            article_cache: HashMap::new(),
            total_articles: 0,
        }
    }

    /// Load a ZIM archive.
    ///
    /// Returns `Err` if the file does not exist or is not a valid openZIM
    /// archive (major version >= 5). On success, returns the article count
    /// read from the real file header.
    pub fn load_archive(&mut self, path: &str) -> Result<u64, String> {
        let path_buf = PathBuf::from(path);
        if !path_buf.is_file() {
            return Err(format!("ZIM archive not found: {path}"));
        }
        let count = self.parse_zim_header(&path_buf)?;
        self.total_articles = count;
        self.archive_path = Some(path_buf);
        Ok(count)
    }

    /// Read the openZIM header (first 70 bytes of the file).
    ///
    /// Layout (per openZIM 5.x spec, little-endian):
    ///   offset 0:  u16 major_version (must be >= 5)
    ///   offset 2:  u16 minor_version
    ///   offset 4:  u16 flags
    ///   offset 6:  u64 cluster_count
    ///   offset 14: u64 article_count
    ///
    /// Only the header is read (70 bytes), NOT the full archive.
    /// Reading article bodies is out of scope for this prototype.
    fn parse_zim_header(&self, path: &PathBuf) -> Result<u64, String> {
        use std::io::Read;
        let mut file = std::fs::File::open(path)
            .map_err(|e| format!("failed to open ZIM archive '{}': {e}", path.display()))?;
        let mut header = [0u8; 70];
        file.read_exact(&mut header)
            .map_err(|e| format!("failed to read ZIM header of '{}': {e}", path.display()))?;
        let major_version = u16::from_le_bytes([header[0], header[1]]);
        if major_version < 5 {
            return Err(format!("not a valid ZIM (major version {major_version})"));
        }
        let article_count = u64::from_le_bytes(header[14..22].try_into().unwrap());
        Ok(article_count)
    }

    /// EXPLICIT demo mode: load the built-in demo articles.
    /// Returns the number of articles loaded. Never invoked implicitly.
    pub fn load_demo(&mut self) -> u64 {
        let demo_articles: [(&str, &[u8]); 6] = [
            ("Premiers_secours", b"Article sur les premiers secours..."),
            ("Survie", b"Techniques de survie en milieu hostile..."),
            (
                "Purification_de_l_eau",
                b"Purifier l'eau par ebullition ou filtration...",
            ),
            (
                "Communication_radio",
                b"Etablir des communications radio de secours...",
            ),
            (
                "Abri_d_urgence",
                b"Construire un abri d'urgence avec des moyens locaux...",
            ),
            (
                "Orientation_cartographique",
                b"Se reperer avec carte et boussole...",
            ),
        ];
        self.article_cache.clear();
        for (title, content) in demo_articles {
            self.article_cache
                .insert(title.to_string(), content.to_vec());
        }
        self.total_articles = demo_articles.len() as u64;
        self.archive_path = None;
        self.total_articles
    }

    /// Search articles by title
    pub fn search(&self, query: &str) -> Vec<String> {
        self.article_cache
            .keys()
            .filter(|title| title.to_lowercase().contains(&query.to_lowercase()))
            .cloned()
            .collect()
    }

    /// Get article content
    pub fn get_article(&self, title: &str) -> Option<&Vec<u8>> {
        self.article_cache.get(title)
    }

    pub fn total_articles(&self) -> u64 {
        self.total_articles
    }
}

impl Default for ZimReader {
    fn default() -> Self {
        Self::new()
    }
}

/*
 * MBTiles Renderer — Offline Vector Maps
 *
 * Renders offline maps from MBTiles (sqlite-based) with OpenMapTiles schema
 * Includes radar view with Geohash positioning
 */

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MapTile {
    pub zoom: u8,
    pub x: u32,
    pub y: u32,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RadarPosition {
    pub latitude: f64,
    pub longitude: f64,
    pub geohash: String,
    pub accuracy_meters: f64,
}

pub struct MBTilesRenderer {
    tile_cache: HashMap<String, Vec<u8>>, // "z/x/y" -> tile data
    db_path: Option<PathBuf>,
}

impl MBTilesRenderer {
    pub fn new() -> Self {
        Self {
            tile_cache: HashMap::new(),
            db_path: None,
        }
    }

    /// Load an MBTiles database.
    ///
    /// Returns `Err` if the file does not exist. Clears any previously cached
    /// demo tiles so that `get_tile` no longer serves the transparent 1x1 PNGs
    /// after switching to a real map database.
    pub fn load(&mut self, path: &str) -> Result<(), String> {
        let path_buf = PathBuf::from(path);
        if !path_buf.is_file() {
            return Err(format!("MBTiles database not found: {path}"));
        }
        // Clear cached demo tiles — the real database is now authoritative
        self.tile_cache.clear();
        // In production: open SQLite and read tiles
        self.db_path = Some(path_buf);
        Ok(())
    }

    /// EXPLICIT demo mode: cache low-zoom tiles (z 0..=5) backed by a valid
    /// transparent 1x1 PNG. Never invoked implicitly.
    pub fn load_demo(&mut self) {
        // The demo PNG is decoded once and reused for every tile
        let png = decode_demo_png();
        for zoom in 0..=5 {
            let num_tiles = 1usize << zoom;
            for x in 0..num_tiles.min(4) {
                for y in 0..num_tiles.min(4) {
                    let key = format!("{zoom}/{x}/{y}");
                    self.tile_cache.insert(key, png.clone());
                }
            }
        }
    }

    /// Get a tile
    pub fn get_tile(&self, zoom: u8, x: u32, y: u32) -> Option<&Vec<u8>> {
        self.tile_cache.get(&format!("{zoom}/{x}/{y}"))
    }

    /// Convert lat/lon to tile coordinates
    pub fn lat_lon_to_tile(lat: f64, lon: f64, zoom: u8) -> (u32, u32) {
        let lat_rad = lat.to_radians();
        let n = 1 << zoom;
        let x = ((lon + 180.0) / 360.0 * n as f64) as u32;
        let y = ((1.0 - (lat_rad.tan() + 1.0 / lat_rad.cos()).ln() / std::f64::consts::PI) / 2.0
            * n as f64) as u32;
        (x, y)
    }

    /// Generate geohash for position
    pub fn position_to_geohash(lat: f64, lon: f64, precision: usize) -> String {
        let base32 = "0123456789bcdefghjkmnpqrstuvwxyz";
        let mut geohash = String::new();

        let mut lat_range = (-90.0, 90.0);
        let mut lon_range = (-180.0, 180.0);
        let mut bits = 0;
        let mut accumulated = 0;

        let mut is_lon = true;
        while geohash.len() < precision {
            if is_lon {
                let mid = (lon_range.0 + lon_range.1) / 2.0;
                if lon >= mid {
                    accumulated = (accumulated << 1) | 1;
                    lon_range.0 = mid;
                } else {
                    accumulated <<= 1;
                    lon_range.1 = mid;
                }
            } else {
                let mid = (lat_range.0 + lat_range.1) / 2.0;
                if lat >= mid {
                    accumulated = (accumulated << 1) | 1;
                    lat_range.0 = mid;
                } else {
                    accumulated <<= 1;
                    lat_range.1 = mid;
                }
            }

            is_lon = !is_lon;
            bits += 1;

            if bits == 5 {
                geohash.push(base32.chars().nth(accumulated & 0x1F).unwrap());
                bits = 0;
                accumulated = 0;
            }
        }

        geohash
    }

    pub fn get_radar_position(&self, lat: f64, lon: f64) -> RadarPosition {
        let geohash = Self::position_to_geohash(lat, lon, 7);
        RadarPosition {
            latitude: lat,
            longitude: lon,
            geohash,
            accuracy_meters: 153.0, // geohash precision 7
        }
    }
}

impl Default for MBTilesRenderer {
    fn default() -> Self {
        Self::new()
    }
}

/// Decode the demo tile payload: a transparent 1x1 PNG (base64), decoded once.
fn decode_demo_png() -> Vec<u8> {
    const DEMO_PNG_B64: &str =
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=";
    base64::engine::general_purpose::STANDARD
        .decode(DEMO_PNG_B64)
        .expect("demo PNG constant must be valid base64")
}

/*
 * IPFS Seeder — Mega-Archive Distribution
 *
 * Desktop nodes seed APKs, ZIM files, AI models to the mesh network
 */

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeedInfo {
    pub cid: String, // Content ID
    pub file_name: String,
    pub file_size_bytes: u64,
    pub mime_type: String,
    /// Whether currently seeding
    pub seeding: bool,
    /// Number of peers served
    pub peer_count: u32,
}

pub struct IpfsSeeder {
    seeds: HashMap<String, SeedInfo>,
    storage_path: PathBuf,
    total_storage_bytes: u64,
}

impl IpfsSeeder {
    /// Create a seeder and ensure the storage directory exists.
    ///
    /// Returns `Err` if the directory cannot be created. No demo seeds are
    /// registered by default — call `register_demo_seeds` explicitly.
    pub fn new(storage_path: &str, max_storage_gb: u64) -> Result<Self, String> {
        let storage_path = PathBuf::from(storage_path);
        fs::create_dir_all(&storage_path).map_err(|e| {
            format!(
                "failed to create IPFS seed directory '{}': {e}",
                storage_path.display()
            )
        })?;

        Ok(Self {
            seeds: HashMap::new(),
            storage_path,
            total_storage_bytes: max_storage_gb * 1024 * 1024 * 1024,
        })
    }

    /// Build a seeder with no seeds and NO directory creation.
    ///
    /// Fallback used when the storage directory is unavailable (e.g. on
    /// read-only filesystems): the seeder stays empty but fully functional.
    pub fn disabled(max_storage_gb: u64) -> Self {
        Self {
            seeds: HashMap::new(),
            storage_path: PathBuf::from("/tmp/onde-ipfs"),
            total_storage_bytes: max_storage_gb * 1024 * 1024 * 1024,
        }
    }

    /// Explicitly register the built-in demo seeds.
    ///
    /// Each seed is only added if it fits within the storage budget
    /// (`used_storage + size <= total_storage`); seeds that exceed the
    /// remaining capacity are skipped. Never invoked implicitly.
    pub fn register_demo_seeds(&mut self) {
        let demo_seeds = [
            (
                "QmWikipedia",
                "wikipedia_fr_2024.zim",
                90_000_000_000u64,
                "application/x-zim",
            ),
            (
                "QmOndeAPK",
                "onde-latest.apk",
                45_000_000u64,
                "application/vnd.android.package-archive",
            ),
            (
                "QmQwen08B",
                "qwen2-0_5b-q4_k_m.gguf",
                530_000_000u64,
                "application/octet-stream",
            ),
            (
                "QmQwen9B",
                "qwen2-7b-q4_k_m.gguf",
                5_600_000_000u64,
                "application/octet-stream",
            ),
            (
                "QmMaps",
                "france_tiles.mbtiles",
                2_000_000_000u64,
                "application/x-sqlite",
            ),
        ];

        for (cid, name, size, mime) in demo_seeds {
            if self.used_storage() + size <= self.total_storage_bytes {
                self.seeds.insert(
                    cid.to_string(),
                    SeedInfo {
                        cid: cid.to_string(),
                        file_name: name.to_string(),
                        file_size_bytes: size,
                        mime_type: mime.to_string(),
                        seeding: true,
                        peer_count: 0,
                    },
                );
            }
        }
    }

    /// The storage directory this seeder uses (may not exist for `disabled`).
    pub fn storage_path(&self) -> &PathBuf {
        &self.storage_path
    }

    pub fn list_seeds(&self) -> Vec<&SeedInfo> {
        self.seeds.values().collect()
    }

    pub fn get_seed(&self, cid: &str) -> Option<&SeedInfo> {
        self.seeds.get(cid)
    }

    pub fn used_storage(&self) -> u64 {
        self.seeds.values().map(|s| s.file_size_bytes).sum()
    }

    /// Remaining storage budget — saturating, never underflows.
    pub fn available_storage(&self) -> u64 {
        self.total_storage_bytes.saturating_sub(self.used_storage())
    }
}

/*
 * Tiered Message Store — Hierarchical storage (Audit #8)
 *
 * Messages are classified in retention tiers instead of keeping everything
 * forever:
 *
 *   Critical  → 7 jours   (alertes civiques, urgences)
 *   Important → 2 jours   (informations vérifiées)
 *   Normal    → 6 heures  (discussions courantes)
 *   Low       → 1 heure   (flux non critiques)
 *
 * Payloads are compressed (flate2/Deflate — pur Rust, aucun binaire natif)
 * to save storage and bandwidth. Geohash sharding decides whether a message
 * is stored locally or only routed onward, so mobile nodes keep only the
 * slices of the mesh they actually care about.
 */

use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use flate2::Compression;
use std::io::{Read, Write};

/// Niveau de rétention d'un message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageTier {
    Critical,
    Important,
    Normal,
    Low,
}

impl MessageTier {
    /// Durée de rétention en secondes.
    pub fn retention_secs(self) -> u64 {
        match self {
            MessageTier::Critical => 7 * 24 * 3600,
            MessageTier::Important => 2 * 24 * 3600,
            MessageTier::Normal => 6 * 3600,
            MessageTier::Low => 3600,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            MessageTier::Critical => "critical",
            MessageTier::Important => "important",
            MessageTier::Normal => "normal",
            MessageTier::Low => "low",
        }
    }
}

/// Un message stocké : métadonnées + payload compressé.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TieredMessage {
    pub id: String,
    pub tier: MessageTier,
    /// Horodatage de réception (unix secs)
    pub created_at: u64,
    /// Payload compressé (Deflate)
    pub payload: Vec<u8>,
    /// Taille originale du payload (pour stats / décompression)
    pub original_size: usize,
    /// Geohash (7 chars) de la zone émettrice — utilisé pour le sharding
    pub geohash: String,
}

/// Profil de stockage selon le type de nœud.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StoragePolicy {
    /// Téléphone — budget serré
    Mobile,
    /// Ordinateur / nœud fixe
    Desktop,
    /// Passerelle mesh avec beaucoup d'espace
    Gateway,
}

impl StoragePolicy {
    pub fn max_bytes(self) -> u64 {
        match self {
            StoragePolicy::Mobile => 64 * 1024 * 1024,         // 64 MB
            StoragePolicy::Desktop => 2 * 1024 * 1024 * 1024,  // 2 GB
            StoragePolicy::Gateway => 16 * 1024 * 1024 * 1024, // 16 GB
        }
    }
}

/// Magasin hiérarchique de messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TieredMessageStore {
    messages: Vec<TieredMessage>,
    policy: StoragePolicy,
    /// Seuil de délégué : au-delà de cette taille, un message mobile est
    /// délégué à un nœud desktop plutôt que stocké localement.
    delegate_threshold_bytes: usize,
    /// Geohash (7 caractères) du nœud local — utilisé par le **sharding
    /// géographique** appliqué dans [`TieredMessageStore::store`] : on ne
    /// retient localement que les messages de notre voisinage (ou les
    /// alertes critiques). Les autres sont simplement routés (non stockés).
    my_geohash: String,
    /// Phase 3.6 — hook de métriques optionnel : jauges `storage.*` tenues
    /// à jour par add/store/restore/purge. Coût : quelques atomiques, hors
    /// du chemin critique (compression déjà dominante dans [`Self::store`]).
    /// Hors sérialisation (runtime-only, re-branché après désérialisation).
    #[serde(skip)]
    metrics: Option<std::sync::Arc<crate::metrics::NodeMetrics>>,
}

impl TieredMessageStore {
    pub fn new(policy: StoragePolicy) -> Self {
        Self::with_geohash(policy, "u09tunq") // position démo (Paris) par défaut
    }

    /// Constructeur complet : politique + geohash local du nœud.
    pub fn with_geohash(policy: StoragePolicy, my_geohash: &str) -> Self {
        Self {
            messages: Vec::new(),
            policy,
            delegate_threshold_bytes: match policy {
                StoragePolicy::Mobile => 64 * 1024,
                StoragePolicy::Desktop => 1024 * 1024,
                StoragePolicy::Gateway => 16 * 1024 * 1024,
            },
            my_geohash: my_geohash.to_string(),
            metrics: None,
        }
    }

    /// Changer la position locale (après un fix GPS, par exemple).
    pub fn set_my_geohash(&mut self, geohash: &str) {
        self.my_geohash = geohash.to_string();
    }

    /// Brancher le registre de métriques du nœud (Phase 3.6). Sans appel,
    /// le magasin fonctionne à l'identique (hook `None`, zéro surcoût).
    pub fn set_metrics(&mut self, metrics: std::sync::Arc<crate::metrics::NodeMetrics>) {
        self.metrics = Some(metrics);
    }

    /// Longueur de préfixe commun exigée pour le stockage local, selon le
    /// profil : un mobile ne garde que son district (~5 km), un desktop sa
    /// région (~39 km), une passerelle un espace large (~156 km).
    /// Les alertes Critical sont toujours stockées, quel que soit le préfixe.
    fn shard_prefix_len(&self) -> usize {
        match self.policy {
            StoragePolicy::Mobile => 5,
            StoragePolicy::Desktop => 4,
            StoragePolicy::Gateway => 3,
        }
    }

    /// Compresser un payload (Deflate).
    fn compress(data: &[u8]) -> Vec<u8> {
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
        encoder
            .write_all(data)
            .expect("in-memory write cannot fail");
        encoder.finish().expect("in-memory finish cannot fail")
    }

    /// Décompresser un payload.
    fn decompress(data: &[u8]) -> Result<Vec<u8>, String> {
        let mut decoder = DeflateDecoder::new(data);
        let mut out = Vec::new();
        decoder.read_to_end(&mut out).map_err(|e| e.to_string())?;
        Ok(out)
    }

    /// Stocker un message dans le tiers approprié (payload compressé).
    ///
    /// Le budget est vérifié sur la taille **brute** (ce que le nœud doit
    /// logiquement retenir) : la compression réduit l'empreinte disque, elle
    /// ne permet pas de dépasser le budget de la politique. Le **sharding
    /// géographique** est appliqué ici : un message hors de notre voisinage
    /// (et non critique) n'est pas stocké localement (`Ok(false)`) — il est
    /// seulement routé. Retourne `Ok(false)` si le budget est dépassé ou si
    /// le sharding exclut le message.
    pub fn store(
        &mut self,
        id: &str,
        tier: MessageTier,
        payload: &[u8],
        created_at: u64,
        geohash: &str,
    ) -> Result<bool, String> {
        // Idempotence par id (Aikido PR#8 MED) : un même événement rejoué
        // (replay de gossip après restart ou éviction de known_events) ne doit
        // pas créer une seconde copie en mémoire/SQLite. Si l'id est déjà
        // stocké, on ne ré-ajoute pas.
        if self.messages.iter().any(|m| m.id == id) {
            return Ok(false);
        }
        if self.used_raw_bytes() + payload.len() as u64 > self.policy.max_bytes() {
            return Ok(false);
        }
        if !Self::should_store_locally(geohash, &self.my_geohash, tier, self.shard_prefix_len()) {
            return Ok(false);
        }
        let compressed = Self::compress(payload);
        // Taille capturée avant move du payload compressé (zéro copie ajoutée).
        let compressed_len = compressed.len();
        self.messages.push(TieredMessage {
            id: id.to_string(),
            tier,
            created_at,
            original_size: payload.len(),
            payload: compressed,
            geohash: geohash.to_string(),
        });
        // Phase 3.6 — jauges storage.* : octets bruts logiques vs compressés.
        if let Some(m) = &self.metrics {
            m.record_storage_added(payload.len() as u64, compressed_len as u64);
        }
        Ok(true)
    }

    /// Récupérer le payload décompressé d'un message.
    pub fn get(&self, id: &str) -> Option<Result<Vec<u8>, String>> {
        self.messages
            .iter()
            .find(|m| m.id == id)
            .map(|m| Self::decompress(&m.payload))
    }

    /// Accès aux messages stockés (pour la persistance SQLite).
    pub fn all_messages(&self) -> &[TieredMessage] {
        &self.messages
    }

    /// Restaurer un message préalablement persisté (au démarrage, après un
    /// crash). Le payload est déjà compressé — on ne le re-compresse pas.
    /// Respecte le budget et dédoublonne par id.
    pub fn restore(&mut self, msg: TieredMessage) -> Result<bool, String> {
        if self.messages.iter().any(|m| m.id == msg.id) {
            return Ok(false);
        }
        if self.used_raw_bytes() + msg.original_size as u64 > self.policy.max_bytes() {
            return Ok(false);
        }
        // Phase 3.6 — jauges storage.* pour la restauration post-crash.
        if let Some(m) = &self.metrics {
            m.record_storage_added(msg.original_size as u64, msg.payload.len() as u64);
        }
        self.messages.push(msg);
        Ok(true)
    }

    /// Supprimer les messages expirés (selon leur tier). Retourne le nombre
    /// de messages purgés.
    pub fn sweep_expired(&mut self, now: u64) -> usize {
        let before = self.messages.len();
        // Phase 3.6 — comptabiliser la purge AVANT retrait pour maintenir
        // exactement les jauges storage.* (sweep rare : coût négligeable).
        let mut purged_count = 0u64;
        let mut purged_raw = 0u64;
        let mut purged_stored = 0u64;
        self.messages.retain(|m| {
            let keep = now.saturating_sub(m.created_at) < m.tier.retention_secs();
            if !keep {
                purged_count += 1;
                purged_raw += m.original_size as u64;
                purged_stored += m.payload.len() as u64;
            }
            keep
        });
        if let Some(m) = &self.metrics {
            m.record_storage_removed(purged_count, purged_raw, purged_stored);
        }
        before - self.messages.len()
    }

    /// Décider si un message doit être stocké localement selon le sharding
    /// Geohash : on garde ce qui est dans notre voisinage (préfixe commun
    /// >= `prefix_len`) ou les alertes critiques.
    pub fn should_store_locally(
        geohash: &str,
        my_geohash: &str,
        tier: MessageTier,
        prefix_len: usize,
    ) -> bool {
        if tier == MessageTier::Critical {
            return true;
        }
        if prefix_len == 0 {
            return false;
        }
        let common = geohash
            .chars()
            .zip(my_geohash.chars())
            .take_while(|(a, b)| a == b)
            .count();
        common >= prefix_len
    }

    /// Un gros payload doit-il être délégué à un nœud desktop ?
    pub fn should_delegate(&self, payload_size: usize) -> bool {
        payload_size > self.delegate_threshold_bytes
    }

    /// Octets stockés (payload compressés) — empreinte disque réelle.
    pub fn used_bytes(&self) -> u64 {
        self.messages.iter().map(|m| m.payload.len() as u64).sum()
    }

    /// Octets bruts logiques (somme des `original_size`) — utilisé pour le
    /// contrôle du budget (la compression est un bonus, pas un contournement).
    pub fn used_raw_bytes(&self) -> u64 {
        self.messages.iter().map(|m| m.original_size as u64).sum()
    }

    /// Nombre de messages par tier (stats).
    pub fn count_by_tier(&self) -> [(MessageTier, usize); 4] {
        [
            (
                MessageTier::Critical,
                self.messages
                    .iter()
                    .filter(|m| m.tier == MessageTier::Critical)
                    .count(),
            ),
            (
                MessageTier::Important,
                self.messages
                    .iter()
                    .filter(|m| m.tier == MessageTier::Important)
                    .count(),
            ),
            (
                MessageTier::Normal,
                self.messages
                    .iter()
                    .filter(|m| m.tier == MessageTier::Normal)
                    .count(),
            ),
            (
                MessageTier::Low,
                self.messages
                    .iter()
                    .filter(|m| m.tier == MessageTier::Low)
                    .count(),
            ),
        ]
    }

    pub fn total_count(&self) -> usize {
        self.messages.len()
    }

    /// Taux de compression moyen (stats / rapport d'audit).
    pub fn compression_stats(&self) -> (usize, usize) {
        let raw: usize = self.messages.iter().map(|m| m.original_size).sum();
        let stored: usize = self.messages.iter().map(|m| m.payload.len()).sum();
        (raw, stored)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- ZIM ----------

    #[test]
    fn test_zim_new_is_empty() {
        let reader = ZimReader::new();
        assert_eq!(reader.total_articles(), 0);
    }

    #[test]
    fn test_zim_load_missing_file_errors() {
        let mut reader = ZimReader::new();
        let missing = std::env::temp_dir().join(format!("missing-zim-{}", std::process::id()));
        assert!(
            reader.load_archive(missing.to_str().unwrap()).is_err(),
            "loading a missing ZIM file must fail loudly"
        );
    }

    #[test]
    fn test_zim_load_real_header() {
        // Synthetic openZIM header: >= 70 bytes, major version 5, articleCount = 12345
        let mut buf = [0u8; 70];
        buf[0..2].copy_from_slice(&5u16.to_le_bytes()); // major version
        buf[14..22].copy_from_slice(&12345u64.to_le_bytes()); // article count
        let path = std::env::temp_dir().join(format!("onde-zim-{}.zim", std::process::id()));
        std::fs::write(&path, buf).unwrap();

        let mut reader = ZimReader::new();
        let count = reader.load_archive(path.to_str().unwrap()).unwrap();
        assert_eq!(count, 12345, "article count must come from the real header");
        assert_eq!(reader.total_articles(), 12345);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_zim_rejects_old_major_version() {
        // >= 70 bytes so the header is fully read, major version 3 (< 5)
        let mut buf = [0u8; 70];
        buf[0..2].copy_from_slice(&3u16.to_le_bytes()); // major version 3
        buf[14..22].copy_from_slice(&100u64.to_le_bytes());
        let path = std::env::temp_dir().join(format!("onde-zim-old-{}.zim", std::process::id()));
        std::fs::write(&path, buf).unwrap();

        let mut reader = ZimReader::new();
        let err = reader.load_archive(path.to_str().unwrap()).unwrap_err();
        assert!(err.contains("not a valid ZIM"), "unexpected error: {err}");
        assert!(
            err.contains("3"),
            "error must mention the major version: {err}"
        );

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_zim_rejects_truncated_header() {
        // A file shorter than the 70-byte header must fail loudly (not read full archive)
        let buf = [0u8; 10];
        let path = std::env::temp_dir().join(format!("onde-zim-trunc-{}.zim", std::process::id()));
        std::fs::write(&path, buf).unwrap();

        let mut reader = ZimReader::new();
        assert!(
            reader.load_archive(path.to_str().unwrap()).is_err(),
            "a truncated ZIM header must be rejected"
        );

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_zim_load_demo() {
        let mut reader = ZimReader::new();
        let count = reader.load_demo();
        assert!(
            count >= 5,
            "demo mode must expose at least 5 articles, got {count}"
        );
        assert_eq!(reader.total_articles(), count);
        assert!(
            !reader.search("secours").is_empty(),
            "search('secours') must find demo articles"
        );
        assert!(reader.get_article("Premiers_secours").is_some());
    }

    // ---------- MBTiles ----------

    #[test]
    fn test_geohash() {
        // Eiffel Tower
        let geohash = MBTilesRenderer::position_to_geohash(48.8584, 2.2945, 7);
        assert_eq!(geohash.len(), 7);
        assert_eq!(geohash, "u09tunq");
    }

    #[test]
    fn test_tile_coords() {
        let (x, y) = MBTilesRenderer::lat_lon_to_tile(48.8584, 2.2945, 5);
        assert!(x < 32 && y < 32);
    }

    #[test]
    fn test_mbtiles_load_missing_file_errors() {
        let mut maps = MBTilesRenderer::new();
        let missing = std::env::temp_dir().join(format!("missing-mbtiles-{}", std::process::id()));
        assert!(
            maps.load(missing.to_str().unwrap()).is_err(),
            "loading a missing MBTiles file must fail loudly"
        );
    }

    #[test]
    fn test_mbtiles_load_existing_file_no_demo_tiles() {
        // An existing file loads OK, but does NOT inject demo tiles
        let path = std::env::temp_dir().join(format!("onde-maps-{}.mbtiles", std::process::id()));
        std::fs::write(&path, b"not a real mbtiles").unwrap();

        let mut maps = MBTilesRenderer::new();
        assert!(maps.load(path.to_str().unwrap()).is_ok());
        assert!(
            maps.get_tile(0, 0, 0).is_none(),
            "load() must not inject demo tiles"
        );

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_mbtiles_demo_tiles_are_valid_png() {
        let mut maps = MBTilesRenderer::new();
        maps.load_demo();

        let tiles: Vec<Vec<u8>> = maps.tile_cache.values().cloned().collect();
        assert!(!tiles.is_empty(), "demo mode must cache tiles");

        for tile in &tiles {
            // PNG signature
            assert!(
                tile.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]),
                "tile must start with the PNG signature, got {} bytes",
                tile.len()
            );
            // IEND chunk present
            assert!(
                tile.windows(8)
                    .any(|w| w == [0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82]),
                "tile must contain the IEND chunk"
            );
            assert!(tile.len() > 60, "demo tile must be a full PNG");
        }

        // Spot-check low-zoom coverage
        assert!(maps.get_tile(0, 0, 0).is_some());
        assert!(maps.get_tile(5, 2, 2).is_some());
    }

    // ---------- IPFS ----------

    #[test]
    fn test_ipfs_new_invalid_path_errors() {
        // A path whose parent is a regular file cannot be created as a directory
        let parent = std::env::temp_dir().join(format!("onde-ipfs-file-{}", std::process::id()));
        std::fs::write(&parent, b"not a dir").unwrap();
        let bad = parent.join("sub");

        let result = IpfsSeeder::new(bad.to_str().unwrap(), 100);
        assert!(
            result.is_err(),
            "seeder must fail when the directory cannot be created"
        );

        std::fs::remove_file(&parent).ok();
    }

    #[test]
    fn test_ipfs_new_empty_no_auto_demo() {
        let path = std::env::temp_dir().join(format!("onde-ipfs-{}", std::process::id()));
        let seeder =
            IpfsSeeder::new(path.to_str().unwrap(), 100).expect("seeder should initialize");
        assert!(
            seeder.list_seeds().is_empty(),
            "no demo seeds may be auto-registered"
        );
        assert_eq!(seeder.available_storage(), 100 * 1024 * 1024 * 1024);

        std::fs::remove_dir_all(&path).ok();
    }

    #[test]
    fn test_ipfs_demo_seeds_fit_100gb() {
        let path = std::env::temp_dir().join(format!("onde-ipfs-demo-{}", std::process::id()));
        let mut seeder = IpfsSeeder::new(path.to_str().unwrap(), 100).unwrap();
        seeder.register_demo_seeds();

        let seeds = seeder.list_seeds();
        assert_eq!(seeds.len(), 5, "all demo seeds fit within 100 GB");

        let total = 100 * 1024 * 1024 * 1024;
        assert_eq!(seeder.available_storage(), total - seeder.used_storage());
        assert!(seeder.available_storage() > 0);
        assert!(seeder.get_seed("QmWikipedia").is_some());

        std::fs::remove_dir_all(&path).ok();
    }

    #[test]
    fn test_ipfs_demo_seeds_partial_1gb() {
        let path = std::env::temp_dir().join(format!("onde-ipfs-small-{}", std::process::id()));
        let mut seeder = IpfsSeeder::new(path.to_str().unwrap(), 1).unwrap();
        seeder.register_demo_seeds();

        // 1 GB fits OndeAPK (45 MB) and Qwen05B (530 MB); Wikipedia (90 GB) is skipped
        let ids: Vec<&str> = seeder.list_seeds().iter().map(|s| s.cid.as_str()).collect();
        assert!(ids.contains(&"QmOndeAPK"), "45 MB seed must fit in 1 GB");
        assert!(ids.contains(&"QmQwen08B"), "530 MB seed must fit in 1 GB");
        assert!(!ids.contains(&"QmWikipedia"), "90 GB seed must be skipped");

        // Saturating arithmetic: available never underflows
        let total = 1024u64 * 1024 * 1024;
        let expected_used = 45_000_000u64 + 530_000_000u64;
        assert_eq!(seeder.available_storage(), total - expected_used);

        std::fs::remove_dir_all(&path).ok();
    }

    #[test]
    fn test_ipfs_disabled() {
        let seeder = IpfsSeeder::disabled(100);
        assert!(seeder.list_seeds().is_empty());
        assert_eq!(seeder.available_storage(), 100 * 1024 * 1024 * 1024);
    }

    // ---------- Tiered store ----------

    #[test]
    fn test_tier_retention() {
        assert_eq!(MessageTier::Critical.retention_secs(), 7 * 24 * 3600);
        assert_eq!(MessageTier::Important.retention_secs(), 2 * 24 * 3600);
        assert_eq!(MessageTier::Normal.retention_secs(), 6 * 3600);
        assert_eq!(MessageTier::Low.retention_secs(), 3600);
    }

    #[test]
    fn test_store_compress_roundtrip() {
        let mut store = TieredMessageStore::new(StoragePolicy::Mobile);
        let payload = b"alerte civique : coupure d'eau secteur nord, reservoir 3 rempli".repeat(10);

        let stored = store
            .store(
                "evt-1",
                MessageTier::Critical,
                &payload,
                1_800_000_000,
                "u09tunq",
            )
            .expect("store must accept within budget");
        assert!(stored);

        // Compression effective (payload répétitif)
        let (raw, compressed) = store.compression_stats();
        assert!(compressed < raw, "compression must reduce size");

        // Round-trip
        let got = store
            .get("evt-1")
            .expect("message must exist")
            .expect("decompress ok");
        assert_eq!(got, payload);
    }

    #[test]
    fn test_sweep_expired_by_tier() {
        let mut store = TieredMessageStore::new(StoragePolicy::Desktop);
        let now = 2_000_000_000u64;
        // Messages locaux (geohash du nœud) — le sharding géographique les
        // retient ; seuls des messages distants non critiques seraient écartés.
        store
            .store(
                "crit",
                MessageTier::Critical,
                b"c",
                now - 2 * 24 * 3600,
                "u09tunq",
            )
            .unwrap();
        store
            .store("norm", MessageTier::Normal, b"n", now - 7 * 3600, "u09tunq")
            .unwrap();
        store
            .store("low", MessageTier::Low, b"l", now - 7200, "u09tunq")
            .unwrap();

        assert_eq!(store.total_count(), 3);
        let purged = store.sweep_expired(now);
        assert_eq!(purged, 2, "Normal (7h > 6h) and Low (2h > 1h) must expire");
        assert_eq!(store.total_count(), 1);
        assert!(store.get("crit").is_some());
        assert!(store.get("norm").is_none());
        assert!(store.get("low").is_none());
    }

    #[test]
    fn test_geohash_sharding() {
        let my = "u09tunq"; // Paris (Eiffel Tower)
                            // Même voisinage (préfixe >= 5) → stockage local
        assert!(TieredMessageStore::should_store_locally(
            "u09tunx",
            my,
            MessageTier::Normal,
            5
        ));
        // Zone éloignée → non stocké (sauf alerte critique)
        assert!(!TieredMessageStore::should_store_locally(
            "sp05abc",
            my,
            MessageTier::Normal,
            5
        ));
        assert!(TieredMessageStore::should_store_locally(
            "sp05abc",
            my,
            MessageTier::Critical,
            5
        ));
        // prefix_len = 0 → rien n'est stocké localement
        assert!(!TieredMessageStore::should_store_locally(
            "u09tunq",
            my,
            MessageTier::Normal,
            0
        ));
    }

    #[test]
    fn test_store_applies_geohash_sharding() {
        // Le sharding doit être câblé dans le flux de stockage réel (store),
        // pas seulement exposé comme fonction utilitaire.
        let mut store = TieredMessageStore::with_geohash(StoragePolicy::Mobile, "u09tunq");
        let payload = b"message de test";

        // 1. Même geohash (message local) → stocké
        assert!(store
            .store("local", MessageTier::Normal, payload, 1_000, "u09tunq")
            .unwrap());
        // 2. Voisinage proche (préfixe 5 commun : u09tu) → stocké
        assert!(store
            .store("near", MessageTier::Normal, payload, 1_000, "u09tuxx")
            .unwrap());
        // 3. Zone éloignée en tier Normal → NON stocké (routé seulement)
        assert!(!store
            .store("far", MessageTier::Normal, payload, 1_000, "sp05abc")
            .unwrap());
        // 4. Zone éloignée mais Critical → toujours stocké (urgence)
        assert!(store
            .store("far-crit", MessageTier::Critical, payload, 1_000, "sp05abc")
            .unwrap());

        assert_eq!(store.total_count(), 3);
        assert!(store.get("local").is_some());
        assert!(store.get("near").is_some());
        assert!(
            store.get("far").is_none(),
            "far Normal message must not be stored locally"
        );
        assert!(
            store.get("far-crit").is_some(),
            "critical alerts are always kept"
        );
    }

    #[test]
    fn test_delegate_threshold() {
        let mobile = TieredMessageStore::new(StoragePolicy::Mobile);
        assert!(
            mobile.should_delegate(70_000),
            "mobile delegates payloads > 64 KB"
        );
        assert!(!mobile.should_delegate(1_000));

        let gateway = TieredMessageStore::new(StoragePolicy::Gateway);
        assert!(
            !gateway.should_delegate(70_000),
            "gateway keeps larger payloads locally"
        );
    }

    #[test]
    fn test_store_respects_budget() {
        let mut store = TieredMessageStore::new(StoragePolicy::Mobile); // 64 MB
        let big = vec![0x55u8; 40 * 1024 * 1024];
        store
            .store("a", MessageTier::Important, &big, 1_000, "u09tunq")
            .unwrap();
        // Deuxième message de 40 Mo : 40+40 = 80 Mo > budget mobile 64 Mo
        // → refusé localement (le budget compte la taille brute, pas compressée)
        let second = store
            .store("b", MessageTier::Important, &big, 1_001, "u09tunq")
            .unwrap();
        assert!(
            !second,
            "second 40 MB message must exceed the 64 MB mobile budget"
        );
        assert_eq!(store.total_count(), 1);
    }
}
