/// Offline Storage — ZIM reader, MBTiles renderer, IPFS seeder
///
/// Honest-storage principle (prototype): a missing file fails loudly with an
/// `Err`. Demo data is only ever loaded through EXPLICIT methods
/// (`load_demo` / `register_demo_seeds`), never silently by default.

use std::collections::HashMap;
use std::path::PathBuf;
use std::fs;
use serde::{Deserialize, Serialize};

use base64::Engine as _;

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
            ("Purification_de_l_eau", b"Purifier l'eau par ebullition ou filtration..."),
            ("Communication_radio", b"Etablir des communications radio de secours..."),
            ("Abri_d_urgence", b"Construire un abri d'urgence avec des moyens locaux..."),
            ("Orientation_cartographique", b"Se reperer avec carte et boussole..."),
        ];
        self.article_cache.clear();
        for (title, content) in demo_articles {
            self.article_cache.insert(title.to_string(), content.to_vec());
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
    pub cid: String,               // Content ID
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
            ("QmWikipedia", "wikipedia_fr_2024.zim", 90_000_000_000u64, "application/x-zim"),
            ("QmOndeAPK", "onde-latest.apk", 45_000_000u64, "application/vnd.android.package-archive"),
            ("QmQwen08B", "qwen2-0_5b-q4_k_m.gguf", 530_000_000u64, "application/octet-stream"),
            ("QmQwen9B", "qwen2-7b-q4_k_m.gguf", 5_600_000_000u64, "application/octet-stream"),
            ("QmMaps", "france_tiles.mbtiles", 2_000_000_000u64, "application/x-sqlite"),
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
        buf[0..2].copy_from_slice(&5u16.to_le_bytes());   // major version
        buf[14..22].copy_from_slice(&12345u64.to_le_bytes()); // article count
        let path = std::env::temp_dir().join(format!("onde-zim-{}.zim", std::process::id()));
        std::fs::write(&path, &buf).unwrap();

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
        buf[0..2].copy_from_slice(&3u16.to_le_bytes());   // major version 3
        buf[14..22].copy_from_slice(&100u64.to_le_bytes());
        let path = std::env::temp_dir().join(format!("onde-zim-old-{}.zim", std::process::id()));
        std::fs::write(&path, &buf).unwrap();

        let mut reader = ZimReader::new();
        let err = reader.load_archive(path.to_str().unwrap()).unwrap_err();
        assert!(err.contains("not a valid ZIM"), "unexpected error: {err}");
        assert!(err.contains("3"), "error must mention the major version: {err}");

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_zim_rejects_truncated_header() {
        // A file shorter than the 70-byte header must fail loudly (not read full archive)
        let buf = [0u8; 10];
        let path = std::env::temp_dir().join(format!("onde-zim-trunc-{}.zim", std::process::id()));
        std::fs::write(&path, &buf).unwrap();

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
        assert!(count >= 5, "demo mode must expose at least 5 articles, got {count}");
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
                tile.windows(8).any(|w| w == [0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82]),
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
        let seeder = IpfsSeeder::new(path.to_str().unwrap(), 100).expect("seeder should initialize");
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
        let total = 1 * 1024 * 1024 * 1024;
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
}
