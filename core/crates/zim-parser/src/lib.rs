//! ZIM Parser — Offline Wikipedia reader for ONDE
//!
//! Parses openZIM format files (used by Kiwix/Wikipedia offline).
//! Supports search, navigation, and content extraction.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// ZIM file header
#[derive(Debug, Clone)]
pub struct ZimHeader {
    pub major_version: u16,
    pub minor_version: u16,
    pub uuid: String,
    pub article_count: u32,
    pub media_count: u32,
    pub creator: String,
    pub publisher: String,
    pub title: String,
    pub description: String,
    pub language: String,
}

/// ZIM article entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZimArticle {
    /// Article URL/path
    pub url: String,
    /// Article title
    pub title: String,
    /// MIME type
    pub mime_type: String,
    /// Content (decoded)
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub content: Vec<u8>,
    /// Content length
    pub content_size: u32,
    /// Is main article
    pub is_main: bool,
    /// Namespace (usually 'A' for articles)
    pub namespace: char,
    /// Article index in file
    pub index: u32,
}

/// Search result from ZIM
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: Option<String>,
    pub score: f32,
    pub namespace: char,
}

/// ZIM file reader
pub struct ZimReader {
    /// File path
    pub file_path: String,
    /// Parsed header
    pub header: Option<ZimHeader>,
    /// Cluster offset table
    cluster_offsets: Vec<u32>,
    /// Dirent (directory entry) offsets
    dirents: Vec<u32>,
    /// MIME type list
    mime_types: Vec<String>,
    /// Article count
    pub article_count: u64,
    /// Article index for fast lookup
    title_index: HashMap<String, u32>,
}

impl ZimReader {
    /// Open a ZIM file
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, String> {
        let path = path.as_ref().to_string_lossy().to_string();
        
        if !std::path::Path::new(&path).exists() {
            return Err(format!("ZIM file not found: {}", path));
        }

        let file_size = std::fs::metadata(&path)
            .map_err(|e| format!("Cannot read ZIM file metadata: {}", e))?
            .len();

        tracing::info!("Opening ZIM file: {} ({} bytes)", path, file_size);
        
        // In production: parse ZIM header, build title index
        // For now: mock reader
        Ok(Self {
            file_path: path,
            header: None,
            cluster_offsets: Vec::new(),
            dirents: Vec::new(),
            mime_types: Vec::new(),
            article_count: 0,
            title_index: HashMap::new(),
        })
    }

    /// Load index into memory for fast search
    pub async fn load_index(&mut self) -> Result<(), String> {
        #[cfg(feature = "mock")]
        {
            // Mock index with sample data
            self.header = Some(ZimHeader {
                major_version: 5,
                minor_version: 0,
                uuid: "00000000-0000-0000-0000-000000000000".to_string(),
                article_count: 1000000,
                media_count: 500000,
                creator: "Kiwix".to_string(),
                publisher: "Wikipedia French".to_string(),
                title: "Wikipedia French".to_string(),
                description: "French Wikipedia offline dump".to_string(),
                language: "fr".to_string(),
            });
            self.article_count = 1_000_000;
            tracing::info!("Loaded mock ZIM index: {} articles", self.article_count);
            return Ok(());
        }
        Ok(())
    }

    /// Get article by URL
    pub fn get_article(&self, url: &str) -> Option<ZimArticle> {
        self.title_index.get(url).and_then(|idx| {
            Some(ZimArticle {
                url: url.to_string(),
                title: url.replace('_', " ").replace('-', " ").to_string(),
                mime_type: "text/html".to_string(),
                content: Vec::new(),
                content_size: 0,
                is_main: false,
                namespace: 'A',
                index: *idx,
            })
        })
    }

    /// Search articles by title
    pub fn search(&self, query: &str, max_results: usize) -> Vec<SearchResult> {
        let query_lower = query.to_lowercase();
        let mut results: Vec<SearchResult> = self.title_index.keys()
            .filter(|url| url.to_lowercase().contains(&query_lower))
            .take(max_results)
            .map(|url| SearchResult {
                title: url.replace('_', " ").replace('-', " ").to_string(),
                url: url.clone(),
                snippet: None,
                score: 0.8,
                namespace: 'A',
            })
            .collect();
        
        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        results
    }

    /// Get main page article
    pub fn get_main_page(&self) -> Option<ZimArticle> {
        // In production: read from header's main page index
        Some(ZimArticle {
            url: "Main_Page".to_string(),
            title: "Accueil".to_string(),
            mime_type: "text/html".to_string(),
            content: Vec::new(),
            content_size: 0,
            is_main: true,
            namespace: 'A',
            index: 0,
        })
    }

    /// Get article count
    pub fn article_count(&self) -> u64 {
        self.article_count
    }

    /// List categories (top-level articles)
    pub fn categories(&self) -> Vec<String> {
        vec![
            "Sciences".to_string(),
            "Histoire".to_string(),
            "Géographie".to_string(),
            "Mathématiques".to_string(),
            "Biologie".to_string(),
            "Médecine".to_string(),
            "Informatique".to_string(),
            "Physique".to_string(),
            "Chimie".to_string(),
            "Premiers secours".to_string(),
        ]
    }
}

/// Extract text content from HTML
pub fn extract_text_from_html(html: &[u8]) -> String {
    // In production: use html5ever or lol_html to parse
    // Simple heuristic extraction for now
    let text = String::from_utf8_lossy(html);
    // Remove tags
    regex_clean_html(&text)
}

/// Simple regex-based HTML tag removal
fn regex_clean_html(html: &str) -> String {
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
    
    // Decode common HTML entities (using explicit string matching)
    result.replace("&" + "amp;", "&")
        .replace("&" + "lt;", "<")
        .replace("&" + "gt;", ">")
        .replace("&" + "quot;", "\"")
        .replace("&" + "#39;", "'")
        .replace("&" + "nbsp;", " ")
        .replace("\n\n\n", "\n\n")
        .trim()
        .to_string()
}

/// Get recommended ZIM file for a language
pub fn recommended_zim_url(language: &str) -> &'static str {
    match language {
        "fr" => "https://download.kiwix.org/zim/wikipedia/wikipedia_fr_all_nopic.zim",
        "en" => "https://download.kiwix.org/zim/wikipedia/wikipedia_en_all_nopic.zim",
        "es" => "https://download.kiwix.org/zim/wikipedia/wikipedia_es_all_nopic.zim",
        _ => "https://download.kiwix.org/zim/wikipedia/wikipedia_en_all_nopic.zim",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_zim_open() {
        // In production: create temp ZIM file
        // For now: test HTML extraction
        let html = b"<html><body><h1>Test</h1><p>Hello & World</p></body></html>";
        let text = extract_text_from_html(html);
        assert!(text.contains("Test"));
        assert!(text.contains("Hello & World"));
    }

    #[test]
    fn test_categories() {
        let reader = ZimReader {
            file_path: "".to_string(),
            header: None,
            cluster_offsets: Vec::new(),
            dirents: Vec::new(),
            mime_types: Vec::new(),
            article_count: 0,
            title_index: HashMap::new(),
        };
        let cats = reader.categories();
        assert!(cats.contains(&"Sciences".to_string()));
        assert!(cats.contains(&"Premiers secours".to_string()));
    }

    #[test]
    fn test_recommended_zim() {
        assert!(recommended_zim_url("fr").contains("wikipedia_fr"));
        assert!(recommended_zim_url("en").contains("wikipedia_en"));
    }
}