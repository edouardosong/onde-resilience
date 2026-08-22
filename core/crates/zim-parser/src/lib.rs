//! ZIM Parser — Offline Wikipedia reader for ONDE
//!
//! Parses openZIM format files (used by Kiwix/Wikipedia offline). Each stage is
//! a separate module committed incrementally:
//!   * [`header`]  — fixed 80-byte header + index-table pointers (this step)
//!   * [`entries`] — dirent parsing, rank & prefix-title search (next step)
//!   * [`content`] — cluster/blob decoding (none / lz4 / zstd) (final step)
//!
//! All external input is bounds-checked and returns typed [`ZimError`] variants
//! rather than panicking.

pub mod header;

use serde::{Deserialize, Serialize};
use std::path::Path;

/// Typed errors for ZIM parsing. Every failure mode is a distinct variant so
/// callers can pattern-match without a `panic!` on the wire.
/// Note: intentionally only `Debug` — the [`ZimError::Io`] variant wraps a
/// non-clone [`std::io::Error`], so `Clone`/`PartialEq` are not derived.
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
            ZimError::UnknownCompression(c) => write!(f, "unknown compression code '{}'", c),
            ZimError::ShortDecompression => write!(f, "short decompression"),
            ZimError::BadMimeIndex(i) => write!(f, "bad MIME index {}", i),
            ZimError::Io(e) => write!(f, "io error: {}", e),
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
    /// Raw dirent offsets located from the URL pointer table (parsed in step 2).
    dirent_offsets: Vec<u32>,
    /// MIME list parsed from the header region.
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
            dirent_offsets: Vec::new(),
            mime_types: Vec::new(),
        })
    }

    /// Parse the fixed header and locate index tables (this stage).
    pub fn load_index(&mut self) -> Result<(), ZimError> {
        let data = std::fs::read(&self.file_path)?;
        let h = header::ZimHeader::parse(&data)?;
        self.header = Some(h.clone());
        self.data = data;

        // Parse the MIME list (null-terminated strings at mime_list_pos).
        let mut mime_types = Vec::new();
        let mut pos = h.mime_list_pos as usize;
        while pos < self.data.len() {
            match self.data[pos..].iter().position(|&b| b == 0) {
                Some(i) => {
                    mime_types.push(String::from_utf8_lossy(&self.data[pos..pos + i]).to_string());
                    pos += i + 1;
                }
                None => break,
            }
        }
        self.mime_types = mime_types;

        // Locate dirent offsets from the URL pointer table (one per article).
        let n = h.article_count as usize;
        let mut dirent_offsets = Vec::with_capacity(n);
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
            dirent_offsets.push(off);
        }
        self.dirent_offsets = dirent_offsets;

        self.loaded = true;
        tracing::info!("Loaded ZIM index: {} articles", h.article_count);
        Ok(())
    }

    /// List the URLs of all located dirents (header-stage utility).
    pub fn article_urls(&self) -> Vec<String> {
        if !self.loaded {
            return Vec::new();
        }
        self.dirent_offsets
            .iter()
            .map(|&off| url_string(self, off))
            .collect()
    }

    /// Resolve an article by exact URL (rank access). Requires [`load_index`]
    /// and step 2 dirent parsing. Stubbed until dirents are decoded.
    pub fn get_article(&self, url: &str) -> Option<ZimArticle> {
        if !self.loaded {
            return None;
        }
        let idx = self
            .dirent_offsets
            .iter()
            .position(|&off| url_string(self, off) == url)?;
        Some(self.build_article(idx))
    }

    /// Prefix-title search: entries whose title starts with `query`. Requires
    /// [`load_index`] and step 2 dirent parsing. Stubbed until dirents decoded.
    pub fn search(&self, query: &str, max_results: usize) -> Vec<SearchResult> {
        if !self.loaded {
            return Vec::new();
        }
        let ql = query.to_lowercase();
        let mut out: Vec<SearchResult> = self
            .dirent_offsets
            .iter()
            .enumerate()
            .filter(|(_, &off)| title_string(self, off).to_lowercase().starts_with(&ql))
            .take(max_results)
            .map(|(_idx, &off)| SearchResult {
                title: title_string(self, off),
                url: url_string(self, off),
                snippet: None,
                score: 0.8,
                namespace: 'A',
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
        let off = self.dirent_offsets[idx];
        let mime = self
            .mime_types
            .get(1)
            .cloned()
            .unwrap_or_else(|| "application/octet-stream".to_string());
        ZimArticle {
            url: url_string(self, off),
            title: title_string(self, off),
            mime_type: mime,
            content: Vec::new(),
            content_size: 0,
            is_main: false,
            namespace: 'A',
            index: idx as u32,
        }
    }

    pub fn article_count(&self) -> u32 {
        self.header.as_ref().map(|h| h.article_count).unwrap_or(0)
    }
}

/// URL = first null-terminated string of a dirent (offset 0).
fn url_string(r: &ZimReader, off: u32) -> String {
    let d = &r.data;
    let start = off as usize;
    if start >= d.len() {
        return String::new();
    }
    let end = d[start..].iter().position(|&b| b == 0).unwrap_or(d.len());
    String::from_utf8_lossy(&d[start..end])
        .trim_matches('\0')
        .to_owned()
}

/// Title = second null-terminated string of a dirent (offset 5, after the ns byte).
fn title_string(r: &ZimReader, off: u32) -> String {
    let d = &r.data;
    let start = (off as usize).saturating_add(5);
    if start >= d.len() {
        return String::new();
    }
    let end = d[start..].iter().position(|&b| b == 0).unwrap_or(d.len());
    String::from_utf8_lossy(&d[start..end])
        .trim_matches('\0')
        .to_owned()
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
}
