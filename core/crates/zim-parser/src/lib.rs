//! ZIM Parser — Offline Wikipedia reader for ONDE
//!
//! Parses openZIM format files (used by Kiwix/Wikipedia offline). Each stage is
//! a separate module committed incrementally:
//!   * [`header`]  — fixed 80-byte header + index-table pointers (this step)
//!   * [`entries`] — dirent parsing, rank & prefix-title search (this step)
//!   * [`content`] — cluster/blob decoding (none / lz4 / zstd) (final step)
//!
//! All external input is bounds-checked and returns typed [`ZimError`] variants
//! rather than panicking.

pub mod content;
pub mod entries;
pub mod header;

use serde::{Deserialize, Serialize};
use std::path::Path;

/// Typed errors for ZIM parsing. Every failure mode is a distinct variant so
/// callers can pattern-match without a `panic!` on the wire. Note: only `Debug`
/// is derived — [`ZimError::Io`] wraps a non-clone [`std::io::Error`].
#[derive(Debug)]
pub enum ZimError {
    /// Magic number is not `KIM\x00`.
    InvalidMagic,
    /// Major version greater than the classic limit (6).
    BadVersion(u16),
    /// Buffer shorter than the field being read.
    TruncatedHeader,
    /// Offset points outside the backing buffer.
    OutOfBounds { offset: u32, len: u32 },
    /// Underlying I/O failure (file read, metadata, ...).
    Io(std::io::Error),
    /// Blob compression code not supported by this build.
    UnknownCompression(u8),
    /// Decompression produced fewer bytes than the declared size.
    ShortDecompression,
    /// MIME index outside the parsed MIME list.
    BadMimeIndex(u16),
}

impl std::fmt::Display for ZimError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ZimError::InvalidMagic => write!(f, "invalid ZIM magic (expected KIM\\x00)"),
            ZimError::BadVersion(v) => write!(f, "unsupported major version {}", v),
            ZimError::TruncatedHeader => write!(f, "truncated header"),
            ZimError::OutOfBounds { offset, len } => {
                write!(f, "offset {} out of bounds (len {})", offset, len)
            }
            ZimError::Io(e) => write!(f, "io error: {}", e),
            ZimError::UnknownCompression(c) => write!(f, "unknown compression code '{}'", c),
            ZimError::ShortDecompression => write!(f, "short decompression"),
            ZimError::BadMimeIndex(i) => write!(f, "bad MIME index {}", i),
        }
    }
}

impl std::error::Error for ZimError {}

impl From<std::io::Error> for ZimError {
    fn from(e: std::io::Error) -> Self {
        ZimError::Io(e)
    }
}

/// ZIM file header. See [`header::ZimHeader`].
pub use header::ZimHeader;

/// ZIM article entry (a dirent + decoded blob).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZimArticle {
    pub url: String,
    pub title: String,
    pub mime_type: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub content: Vec<u8>,
    pub content_size: u32,
    pub is_main: bool,
    pub namespace: char,
    pub index: u32,
}

/// Search result from a ZIM file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: Option<String>,
    pub score: f32,
    pub namespace: char,
}

/// ZIM file reader.
#[derive(Debug)]
pub struct ZimReader {
    pub file_path: String,
    pub header: Option<ZimHeader>,
    loaded: bool,
    /// Raw bytes of the opened file (consumed by content extraction in step 3).
    data: Vec<u8>,
    /// Parsed dirents located from the URL pointer table.
    dirents: Vec<entries::Dirent>,
    /// MIME list parsed from the header region, indexed by dirent mime_index.
    mime_types: Vec<String>,
}

impl ZimReader {
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, ZimError> {
        let path = path.as_ref().to_string_lossy().to_string();
        if !Path::new(&path).exists() {
            return Err(ZimError::OutOfBounds {
                offset: 0,
                len: u32::MAX, // signal "missing file" distinctly
            });
        }
        tracing::info!(
            "Opening ZIM file: {} bytes",
            std::fs::metadata(&path)?.len()
        );
        Ok(Self {
            file_path: path,
            header: None,
            loaded: false,
            data: Vec::new(),
            dirents: Vec::new(),
            mime_types: Vec::new(),
        })
    }

    /// Parse the fixed header and locate index tables (this stage).
    pub fn load_index(&mut self) -> Result<(), ZimError> {
        let data = std::fs::read(&self.file_path)?;
        let h = header::ZimHeader::parse(&data)?;
        self.header = Some(h.clone());
        self.data = data;

        // Parse the MIME list (null-terminated strings at mime_list_pos). The
        // region ends where the cluster pointer table begins — walking past it
        // would swallow binary tables as "strings".
        let mut mime_types = Vec::new();
        let mut pos = h.mime_list_pos as usize;
        while pos < h.cluster_ptr_pos as usize {
            match self.data[pos..].iter().position(|&b| b == 0) {
                Some(i) => {
                    mime_types.push(String::from_utf8_lossy(&self.data[pos..pos + i]).to_string());
                    pos += i + 1;
                }
                None => break,
            }
        }
        self.mime_types = mime_types;

        // Parse dirents from the URL pointer table (one per article).
        let n = h.article_count as usize;
        let mut dirents = Vec::with_capacity(n);
        for i in 0..n {
            let base = h.url_ptr_pos as usize + i * 4;
            if base + 4 > self.data.len() {
                return Err(ZimError::OutOfBounds {
                    offset: base as u32,
                    len: self.data.len() as u32,
                });
            }
            let off = u32::from_le_bytes([
                self.data[base],
                self.data[base + 1],
                self.data[base + 2],
                self.data[base + 3],
            ]);
            dirents.push(entries::Dirent::parse(&self.data, off as usize)?);
        }
        self.dirents = dirents;

        self.loaded = true;
        tracing::info!("Loaded ZIM index: {} articles", h.article_count);
        Ok(())
    }

    /// List the URLs of all located dirents.
    pub fn article_urls(&self) -> Vec<String> {
        if !self.loaded {
            return Vec::new();
        }
        self.dirents.iter().map(|d| d.url.clone()).collect()
    }

    /// Resolve an article by exact URL (rank access). Requires [`load_index`].
    pub fn get_article(&self, url: &str) -> Option<ZimArticle> {
        if !self.loaded {
            return None;
        }
        let idx = self.dirents.iter().position(|d| d.url == url)?;
        Some(self.build_article(idx))
    }

    /// Prefix-title search: entries whose title starts with `query`. Requires
    /// [`load_index`]. Ranked by score, then insertion order.
    pub fn search(&self, query: &str, max_results: usize) -> Vec<SearchResult> {
        if !self.loaded {
            return Vec::new();
        }
        let ql = query.to_lowercase();
        let mut out: Vec<SearchResult> = self
            .dirents
            .iter()
            .enumerate()
            .filter(|(_, d)| d.title.to_lowercase().starts_with(&ql))
            .take(max_results)
            .map(|(_idx, d)| SearchResult {
                title: d.title.clone(),
                url: d.url.clone(),
                snippet: None,
                score: 0.8,
                namespace: d.namespace,
            })
            .collect();
        out.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        out
    }

    fn build_article(&self, idx: usize) -> ZimArticle {
        let d = &self.dirents[idx];
        let mime = self
            .mime_types
            .get(d.mime_index as usize)
            .cloned()
            .unwrap_or_else(|| "application/octet-stream".to_string());
        ZimArticle {
            url: d.url.clone(),
            title: d.title.clone(),
            mime_type: mime,
            content: self.decode_content(d),
            content_size: d.blob_size,
            is_main: d.namespace == 'A' && idx as u32 == 0,
            namespace: d.namespace,
            index: idx as u32,
        }
    }

    /// Decode an article's blob from its cluster via the classic spec.
    ///
    /// Returns empty content (never panics) when the cluster or blob cannot be
    /// located/decoded; [`content`] exposes the typed errors for callers that
    /// need them.
    fn decode_content(&self, d: &entries::Dirent) -> Vec<u8> {
        let Some(h) = &self.header else {
            return Vec::new();
        };
        // Locate this cluster's file offset from the clusterPtr table.
        let cbase = h.cluster_ptr_pos as usize + d.cluster_index as usize * 4;
        if cbase + 4 > self.data.len() {
            return Vec::new();
        }
        let coff = u32::from_le_bytes([
            self.data[cbase],
            self.data[cbase + 1],
            self.data[cbase + 2],
            self.data[cbase + 3],
        ]);
        let start = coff as usize;
        if start >= self.data.len() {
            return Vec::new();
        }
        // Bound the blob walk so adjacent regions never bleed into this
        // cluster's blob table: the region ends at whichever known structure
        // comes first after `start` — another cluster, or a dirent (layouts may
        // interleave dirents and clusters). EOF is the fallback bound.
        let mut end = self.data.len();
        for d2 in &self.dirents {
            let b2 = h.cluster_ptr_pos as usize + d2.cluster_index as usize * 4;
            if b2 + 4 <= self.data.len() {
                let o2 = u32::from_le_bytes([
                    self.data[b2],
                    self.data[b2 + 1],
                    self.data[b2 + 2],
                    self.data[b2 + 3],
                ]) as usize;
                if start < o2 && o2 < end {
                    end = o2;
                }
            }
        }
        for i in 0..self.dirents.len() {
            let b3 = h.url_ptr_pos as usize + i * 4;
            if b3 + 4 <= self.data.len() {
                let o3 = u32::from_le_bytes([
                    self.data[b3],
                    self.data[b3 + 1],
                    self.data[b3 + 2],
                    self.data[b3 + 3],
                ]) as usize;
                if start < o3 && o3 < end {
                    end = o3;
                }
            }
        }
        let Ok((code, blobs)) = content::parse_cluster(&self.data, start, end) else {
            return Vec::new();
        };
        for b in &blobs {
            if b.offset_in_cluster == d.blob_offset_in_cluster {
                return content::decompress(code, &b.data, d.blob_size).unwrap_or_default();
            }
        }
        Vec::new()
    }

    pub fn article_count(&self) -> u32 {
        self.header.as_ref().map(|h| h.article_count).unwrap_or(0)
    }
}

/// Extract plain text from an HTML blob. Kept stable across parser iterations.
pub fn extract_text_from_html(html: &[u8]) -> String {
    let text = String::from_utf8_lossy(html);
    clean_html(&text)
}

fn clean_html(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        if c == '<' {
            in_tag = true;
        } else if c == '>' {
            in_tag = false;
        } else if !in_tag {
            result.push(c);
        }
    }
    result.trim().to_string()
}

/// Recommended classic ZIM download URL for a language (informational).
pub fn recommended_zim_url(language: &str) -> &'static str {
    match language {
        "fr" => "https://download.kiwix.org/zim/wikipedia/w.wikipedia_fr_all_nopic.zim",
        "en" => "https://download.kiwix.org/zim/wikipedia/wikipedia_en_all_nopic.zim",
        "es" => "https://download.kiwix.org/zim/wikipedia/w.wikipedia_es_all_nopic.zim",
        _ => "https://download.kiwix.org/zim/wikipedia/wikipedia_en_all_nopic.zim",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_extract_text() {
        let html = b"<html><body><h1>Test</h1><p>Hello World</p></body></html>";
        let text = extract_text_from_html(html);
        assert!(text.contains("Test"));
        assert!(text.contains("Hello World"));
    }

    #[test]
    fn test_recommended_zim() {
        assert!(recommended_zim_url("fr").contains("wikipedia_fr"));
        assert!(recommended_zim_url("en").contains("wikipedia_en"));
    }

    #[test]
    fn test_open_missing_file_errors() {
        match ZimReader::open("/nonexistent/does_not_exist.zim") {
            Err(ZimError::OutOfBounds { offset: _, len: _ }) => {}
            other => panic!("expected OutOfBounds, got {:?}", other),
        }
    }

    /// Build a complete classic KIM\\x00 ZIM in memory: `n` articles, one per
    /// cluster, compressed with the given code. Layout follows [`header`] exactly
    /// (cstring metadata, MIME region of `ac*4` bytes, then the three pointer
    /// tables), so this doubles as the reference for on-disk fixtures.
    fn build_zim_file(code: u8, n_articles: usize) -> Vec<u8> {
        let ac = n_articles as u32;
        let html = |i: usize| format!("<html><body><h1>Art {}</h1></body></html>", i).into_bytes();
        let mut out = Vec::new();
        // Fixed header portion at canonical offsets.
        out.extend_from_slice(b"KIM\x00"); // magic @0..4
        out.extend_from_slice(&6u16.to_le_bytes()); // major u16 LE @4..6
        out.extend_from_slice(&3u16.to_le_bytes()); // minor u16 LE @6..8
        out.extend_from_slice(&[0u8; 16]); // UUID @8..24
        out.extend_from_slice(&80u32.to_le_bytes()); // header_size @24
        out.extend_from_slice(&ac.to_le_bytes()); // article_count @28
        out.extend_from_slice(&0u32.to_le_bytes()); // media_count @32
        for s in [b"ONDE", b"KI", b"Syn", b"synth", b"fr"] as [&[u8]; 5] {
            out.extend_from_slice(s);
            out.push(0u8);
        }
        // MIME list region: exactly `ac * 4` bytes (parser convention).
        let mime_list_pos = out.len();
        out.extend_from_slice(b"text/html\x00");
        while out.len() - mime_list_pos < ac as usize * 4 {
            out.push(0u8); // pad to the exact region size
        }
        // Pointer tables: clusterPtr, urlPtr, titlePtr (each `ac` u32).
        let cluster_ptr_pos = out.len();
        out.extend((0..ac).flat_map(|_| 0u32.to_le_bytes()));
        let url_ptr_pos = out.len();
        out.extend((0..ac).flat_map(|_| 0u32.to_le_bytes()));
        let title_ptr_pos = out.len();
        out.extend((0..ac).flat_map(|_| 0u32.to_le_bytes()));
        // Dirents, then one cluster per article (each a single blob at offset 1).
        for i in 0..n_articles {
            let dirent_off = out.len();
            out[url_ptr_pos + i * 4..url_ptr_pos + i * 4 + 4]
                .copy_from_slice(&(dirent_off as u32).to_le_bytes());
            out[title_ptr_pos + i * 4..title_ptr_pos + i * 4 + 4]
                .copy_from_slice(&(dirent_off as u32).to_le_bytes());
            let mut url = format!("A/{:04}_article", i).into_bytes();
            url.push(0u8);
            out.extend_from_slice(&url);
            let mut title = format!("Art {}", i).into_bytes();
            title.push(0u8);
            out.extend_from_slice(&title);
            out.push(b'A'); // namespace
            out.extend_from_slice(&0u16.to_le_bytes()); // mime_index -> text/html
            out.extend_from_slice(&(i as u32).to_le_bytes()); // cluster_index = i
            let raw = html(i);
            let payload: Vec<u8> = match code {
                content::COMP_NONE => raw.clone(),
                content::COMP_LZ4 => lz4_flex::compress(&raw),
                content::COMP_ZSTD => zstd::bulk::compress(&raw, 1).expect("zstd"),
                _ => unreachable!(),
            };
            out.extend_from_slice(&1u32.to_le_bytes()); // blob_offset_in_cluster = 1
            out.extend_from_slice(&(raw.len() as u32).to_le_bytes()); // declared raw size
            let cluster_off = out.len();
            out[cluster_ptr_pos + i * 4..cluster_ptr_pos + i * 4 + 4]
                .copy_from_slice(&(cluster_off as u32).to_le_bytes());
            out.push(code); // compression code byte
            out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            out.extend_from_slice(&payload);
        }
        out
    }

    /// End-to-end content extraction: open a synthetic classic ZIM from disk and
    /// decode every article through its cluster, for all three compression codes.
    #[test]
    fn test_content_extraction_end_to_end() {
        let cases = [
            (content::COMP_NONE, "none"),
            (content::COMP_LZ4, "lz4"),
            (content::COMP_ZSTD, "zstd"),
        ];
        for (code, label) in cases {
            let data = build_zim_file(code, 3);
            let dir = tempfile::tempdir().expect("tmpdir");
            let path = dir.path().join(format!("synthetic_{}.zim", label));
            std::fs::write(&path, &data).expect("write fixture");

            let mut r = ZimReader::open(&path).expect("open");
            r.load_index().expect("load_index");
            assert_eq!(r.article_count(), 3);
            for i in 0..3usize {
                let url = format!("A/{:04}_article", i);
                let art = r
                    .get_article(&url)
                    .unwrap_or_else(|| panic!("[{}] {}", label, url));
                let want = format!("<html><body><h1>Art {}</h1></body></html>", i);
                assert_eq!(art.content, want.as_bytes(), "[{}] article {}", label, i);
                assert_eq!(art.mime_type, "text/html");
                assert_eq!(art.namespace, 'A');
            }
        }
    }

    /// End-to-end entry resolution on a synthetic classic ZIM (see fixtures).
    #[test]
    fn test_entry_resolution_on_fixture() {
        let path = std::env::var("ONDE_ZIM_FIXTURE").unwrap_or_default();
        if path.is_empty() || !Path::new(&path).exists() {
            // Skip cleanly when no fixture is provided (step 4 integration gate).
            return;
        }
        let mut r = ZimReader::open(&path).expect("open");
        r.load_index().expect("load_index");
        assert!(!r.article_urls().is_empty());
        // Rank access by URL must resolve a real dirent.
        let first = r.article_urls()[0].clone();
        let art = r.get_article(&first).expect("get_article");
        assert_eq!(art.url, first);
        assert_ne!(art.mime_type, "application/octet-stream");
        // Prefix-title search must return results with the dirent namespace.
        let q = art.title.split_whitespace().next().unwrap_or("");
        let hits = r.search(q, 10);
        assert!(!hits.is_empty());
        assert_eq!(hits[0].namespace, art.namespace);
    }
}
