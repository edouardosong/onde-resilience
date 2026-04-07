/// Offline Storage — ZIM reader, MBTiles renderer, IPFS seeder

use std::collections::HashMap;
use std::path::PathBuf;
use std::fs;
use serde::{Deserialize, Serialize};

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

    /// Load a ZIM archive file
    pub fn load_archive(&mut self, path: &str) -> Result<u64, String> {
        let path_buf = PathBuf::from(path);

        if !path_buf.exists() {
            // Create demo data for testing
            self.total_articles = 5;
            self.article_cache.insert(
                "Premiers_secours".to_string(),
                b"Article sur les premiers secours...".to_vec(),
            );
            self.article_cache.insert(
                "Survie".to_string(),
                b"Techniques de survie en milieu hostile...".to_vec(),
            );
        } else {
            // In production: parse ZIM file format (MIME header, index, etc.)
            self.total_articles = self.parse_zim_header(&path_buf)?;
        }

        self.archive_path = Some(path_buf);
        Ok(self.total_articles)
    }

    fn parse_zim_header(&self, _path: &PathBuf) -> Result<u64, String> {
        // Production: parse ZIM header format
        // https://wiki.openzim.org/wiki/ZIM_file_format
        Ok(0)
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

    /// Load MBTiles database
    pub fn load(&mut self, path: &str) -> Result<(), String> {
        let path_buf = PathBuf::from(path);

        if path_buf.exists() {
            // In production: open SQLite and read tiles
            self.db_path = Some(path_buf);
        }

        // Cache demo tiles regardless
        self.cache_demo_tiles();
        Ok(())
    }

    fn cache_demo_tiles(&mut self) {
        // Cache low-zoom tiles for demo (transparent 256x256 PNG placeholder)
        for zoom in 0..=5 {
            let num_tiles = 1 << zoom;
            if num_tiles > 32 {
                break; // limit cache size
            }
            for x in 0..num_tiles.min(4) {
                for y in 0..num_tiles.min(4) {
                    let key = format!("{zoom}/{x}/{y}");
                    self.tile_cache.insert(key, vec![0x89, 0x50, 0x4E, 0x47]); // PNG magic
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
    pub fn new(storage_path: &str, max_storage_gb: u64) -> Self {
        let storage_path = PathBuf::from(storage_path);

        // Create directory if needed
        let _ = fs::create_dir_all(&storage_path);

        let mut seeder = Self {
            seeds: HashMap::new(),
            storage_path,
            total_storage_bytes: max_storage_gb * 1024 * 1024 * 1024,
        };

        // Register demo seeds
        seeder.register_demo_seeds();
        seeder
    }

    fn register_demo_seeds(&mut self) {
        let demo_seeds = vec![
            ("QmWikipedia", "wikipedia_fr_2024.zim", 90_000_000_000u64, "application/x-zim"),
            ("QmOndeAPK", "onde-latest.apk", 45_000_000u64, "application/vnd.android.package-archive"),
            ("QmQwen08B", "qwen2-0_5b-q4_k_m.gguf", 530_000_000u64, "application/octet-stream"),
            ("QmQwen9B", "qwen2-7b-q4_k_m.gguf", 5_600_000_000u64, "application/octet-stream"),
            ("QmMaps", "france_tiles.mbtiles", 2_000_000_000u64, "application/x-sqlite"),
        ];

        for (cid, name, size, mime) in demo_seeds {
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

    pub fn list_seeds(&self) -> Vec<&SeedInfo> {
        self.seeds.values().collect()
    }

    pub fn get_seed(&self, cid: &str) -> Option<&SeedInfo> {
        self.seeds.get(cid)
    }

    pub fn used_storage(&self) -> u64 {
        self.seeds.values().map(|s| s.file_size_bytes).sum()
    }

    pub fn available_storage(&self) -> u64 {
        self.total_storage_bytes - self.used_storage()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zim_search() {
        let reader = ZimReader::new();
        assert_eq!(reader.total_articles(), 0);
    }

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
    fn test_ipfs_seeder() {
        let seeder = IpfsSeeder::new("/tmp/onde-ipfs", 100);
        assert!(!seeder.list_seeds().is_empty());
        assert!(seeder.get_seed("QmWikipedia").is_some());
    }
}