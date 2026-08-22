//! ZIM header parsing — fixed 80-byte openZIM header.
//!
//! Classic magic is `KIM\x00` (bytes `4B 49 4D 00`, little-endian u32 = 0x004D494B).
//! Major version for classic files is <= 6. All offsets are bounds-checked and
//! return typed [`crate::ZimError`] variants — no panics on malformed input.

use crate::ZimError;

/// Fixed fields of a classic openZIM header (offsets follow the openZIM spec).
#[derive(Debug, Clone)]
pub struct ZimHeader {
    pub major_version: u16,
    pub minor_version: u16,
    /// 16-byte UUID stored verbatim in the header.
    pub uuid: [u8; 16],
    pub header_size: u32,
    pub article_count: u32,
    pub media_count: u32,
    pub creator: String,
    pub publisher: String,
    pub title: String,
    pub description: String,
    pub language: String,
    /// Position of the MIME list (array of null-terminated strings).
    pub mime_list_pos: u32,
    /// Position of the cluster pointer table (`article_count` u32 entries).
    pub cluster_ptr_pos: u32,
    /// Position of the URL pointer table (`article_count` u32 entries -> dirents).
    pub url_ptr_pos: u32,
    /// Position of the Title pointer table (`article_count` u32 entries -> dirents).
    pub title_ptr_pos: u32,
}

impl ZimHeader {
    /// Parse the fixed portion of a classic openZIM header.
    ///
    /// Reads magic/version/UUID at their canonical offsets, validates them, then
    /// walks the trailing null-terminated strings to locate the four index tables.
    pub fn parse(data: &[u8]) -> Result<Self, ZimError> {
        // Magic number "KIM\x00" at offset 0..4. A partial magic is truncated;
        // a complete but wrong magic is invalid.
        let magic = data.get(0..4).ok_or(ZimError::TruncatedHeader)?;
        if magic.len() != 4 {
            return Err(ZimError::TruncatedHeader);
        }
        if *magic != *b"KIM\x00" {
            return Err(ZimError::InvalidMagic);
        }

        // Major/minor version at offset 4..8 (little-endian u16 each).
        let ver = data.get(4..8).ok_or(ZimError::TruncatedHeader)?;
        if ver.len() != 4 {
            return Err(ZimError::TruncatedHeader);
        }
        let major = u16::from_le_bytes([ver[0], ver[1]]);
        let minor = u16::from_le_bytes([ver[2], ver[3]]);
        // Classic ZIM files use major version <= 6.
        if major > 6 {
            return Err(ZimError::BadVersion(major));
        }

        // UUID at offset 8..24 (exactly 16 bytes).
        let uuid = data.get(8..24).ok_or(ZimError::TruncatedHeader)?;
        if uuid.len() != 16 {
            return Err(ZimError::TruncatedHeader);
        }
        let mut uuid_arr = [0u8; 16];
        uuid_arr.copy_from_slice(uuid);

        // Fixed u32 fields at offset 24..36 (exactly 12 bytes).
        let fixed = data.get(24..36).ok_or(ZimError::TruncatedHeader)?;
        if fixed.len() != 12 {
            return Err(ZimError::TruncatedHeader);
        }
        let header_size = u32::from_le_bytes([fixed[0], fixed[1], fixed[2], fixed[3]]);
        let article_count = u32::from_le_bytes([fixed[4], fixed[5], fixed[6], fixed[7]]);
        let media_count = u32::from_le_bytes([fixed[8], fixed[9], fixed[10], fixed[11]]);

        // Walk the five trailing null-terminated strings (creator, publisher,
        // title, description, language) to locate the index tables.
        let pos = 36usize;
        let read_cstr = |data: &[u8], start: usize| -> Result<(String, usize), ZimError> {
            let end = data[start..]
                .iter()
                .position(|&b| b == 0)
                .map(|i| start + i)
                .ok_or(ZimError::TruncatedHeader)?;
            let s = String::from_utf8_lossy(&data[start..end]).into_owned();
            Ok((s, end + 1))
        };
        let (creator, p1) = read_cstr(data, pos)?;
        let (publisher, p2) = read_cstr(data, p1)?;
        let (title, p3) = read_cstr(data, p2)?;
        let (description, p4) = read_cstr(data, p3)?;
        let (language, p5) = read_cstr(data, p4)?;

        // After the language string: MIME list, then cluster/url/title pointer
        // tables in canonical order.
        let mime_list_pos = p5 as u32;
        let cluster_ptr_pos = mime_list_pos + article_count * 4;
        let url_ptr_pos = cluster_ptr_pos + article_count * 4;
        let title_ptr_pos = url_ptr_pos + article_count * 4;

        Ok(ZimHeader {
            major_version: major,
            minor_version: minor,
            uuid: uuid_arr,
            header_size,
            article_count,
            media_count,
            creator,
            publisher,
            title,
            description,
            language,
            mime_list_pos,
            cluster_ptr_pos,
            url_ptr_pos,
            title_ptr_pos,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal classic header buffer of `size` bytes for truncation tests.
    fn build_header(size: usize) -> Vec<u8> {
        let mut buf = vec![0u8; size];
        if buf.len() >= 4 {
            buf[0..4].copy_from_slice(b"KIM\x00");
        }
        if buf.len() >= 8 {
            buf[4..6].copy_from_slice(&6u16.to_le_bytes()); // major version
            buf[6..8].copy_from_slice(&3u16.to_le_bytes()); // minor version
        }
        if buf.len() >= 28 {
            buf[24] = 80_u8;
            let ac: u32 = 1;
            buf[28..32].copy_from_slice(&ac.to_le_bytes());
        }
        // strings: creator/publisher/title/description/language null-terminated
        if buf.len() >= 60 {
            let strs: [&[u8]; 5] = [b"ONDE", b"KI", b"Syn", b"synth", b"fr"];
            let mut p = 36usize;
            for s in strs {
                let n = s.len();
                buf[p..p + n].copy_from_slice(s);
                buf[p + n] = 0; // null terminator between strings
                p += n + 1;
            }
        }
        buf
    }

    #[test]
    fn valid_header_parses() {
        let data = build_header(80);
        let h = ZimHeader::parse(&data).expect("valid header");
        assert_eq!(h.major_version, 6);
        assert_eq!(h.minor_version, 3);
        assert_eq!(h.header_size, 80);
        assert_eq!(h.article_count, 1);
        assert_eq!(h.creator, "ONDE");
        assert_eq!(h.language, "fr");
        // pointer tables must be ordered and follow the MIME list.
        assert!(h.cluster_ptr_pos >= h.mime_list_pos);
        assert!(h.url_ptr_pos > h.cluster_ptr_pos);
        assert!(h.title_ptr_pos > h.url_ptr_pos);
    }

    #[test]
    fn bad_magic_rejected() {
        let mut data = build_header(80);
        data[0] = b'X'; // corrupt magic
        match ZimHeader::parse(&data) {
            Err(ZimError::InvalidMagic) => {}
            other => panic!("expected InvalidMagic, got {:?}", other),
        }
    }

    #[test]
    fn bad_version_rejected() {
        let mut data = build_header(80);
        data[4] = 7; // major > 6
        match ZimHeader::parse(&data) {
            Err(ZimError::BadVersion(7)) => {}
            other => panic!("expected BadVersion(7), got {:?}", other),
        }
    }

    #[test]
    fn truncated_header_errors() {
        // 20-byte buffer: magic ok, but strings/fields beyond offset 24 missing.
        let data = build_header(20);
        match ZimHeader::parse(&data) {
            Err(ZimError::TruncatedHeader) => {}
            other => panic!("expected TruncatedHeader, got {:?}", other),
        }
    }

    #[test]
    fn tiny_buffer_errors() {
        let data = vec![0u8; 3]; // only partial magic
        match ZimHeader::parse(&data) {
            Err(ZimError::TruncatedHeader) => {}
            other => panic!("expected TruncatedHeader, got {:?}", other),
        }
    }

    #[test]
    fn errors_are_display() {
        // Ensure typed errors implement Display + Error (no panic path).
        let e = ZimError::InvalidMagic;
        let s = format!("{}", e);
        assert!(!s.is_empty());
        let _e: &dyn std::error::Error = &e;
    }
}
