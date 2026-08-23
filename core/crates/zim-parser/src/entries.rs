//! ZIM entry (dirent) resolution — the classic openZIM dirent layout.
//!
//! A dirent is laid out as:
//! ```text
//! url (null-terminated string)
//! title (null-terminated string)
//! namespace (1 byte char)
//! mime_index      (u16 LE)
//! cluster_index   (u32 LE)
//! blob_offset     (u32 LE, offset within the cluster)
//! blob_size       (u32 LE)
//! ```
//! Fields are read sequentially with bounds checks; malformed input yields
//! typed [`crate::ZimError`] variants rather than panics.

use crate::ZimError;

/// A parsed ZIM dirent: one article or media entry.
#[derive(Debug, Clone, PartialEq)]
pub struct Dirent {
    pub url: String,
    pub title: String,
    pub namespace: char,
    pub mime_index: u16,
    pub cluster_index: u32,
    pub blob_offset_in_cluster: u32,
    pub blob_size: u32,
}

impl Dirent {
    /// Parse a dirent located at byte `off` within `data`.
    pub fn parse(data: &[u8], off: usize) -> Result<Self, ZimError> {
        // A pointer table entry may point past EOF (corrupt file): typed error,
        // never an out-of-range slice panic.
        if off >= data.len() {
            return Err(ZimError::OutOfBounds {
                offset: off as u32,
                len: data.len() as u32,
            });
        }
        // URL (null-terminated).
        let url_end = data[off..]
            .iter()
            .position(|&b| b == 0)
            .ok_or(ZimError::TruncatedHeader)?;
        if off + url_end >= data.len() {
            return Err(ZimError::OutOfBounds {
                offset: off as u32,
                len: data.len() as u32,
            });
        }
        let url = String::from_utf8_lossy(&data[off..off + url_end])
            .trim_matches('\0')
            .to_string();
        // Title starts right after the URL's null terminator.
        let t_start = off + url_end + 1;
        let t_end = data[t_start..]
            .iter()
            .position(|&b| b == 0)
            .ok_or(ZimError::TruncatedHeader)?;
        if t_start + t_end >= data.len() {
            return Err(ZimError::OutOfBounds {
                offset: t_start as u32,
                len: data.len() as u32,
            });
        }
        let title = String::from_utf8_lossy(&data[t_start..t_start + t_end])
            .trim_matches('\0')
            .to_string();

        // Namespace (single byte), one past the title's null terminator.
        let ns_off = t_start + t_end + 1;
        let namespace = data.get(ns_off).copied().ok_or(ZimError::TruncatedHeader)? as char;

        // Fixed-width little-endian fields: mime_index (u16), then three u32s.
        let mut q = ns_off + 1;
        let mime_index = read_u16(data, q)?;
        q += 2;
        let cluster_index = read_u32(data, q)?;
        q += 4;
        let blob_offset = read_u32(data, q)?;
        q += 4;
        let blob_size = read_u32(data, q)?;
        q += 4;

        // All fields consumed within the buffer (defensive bounds check).
        if q > data.len() {
            return Err(ZimError::OutOfBounds {
                offset: q as u32,
                len: data.len() as u32,
            });
        }

        Ok(Dirent {
            url,
            title,
            namespace,
            mime_index,
            cluster_index,
            blob_offset_in_cluster: blob_offset,
            blob_size,
        })
    }
}

/// Read a little-endian u16 at `p`, bounds-checked.
fn read_u16(data: &[u8], p: usize) -> Result<u16, ZimError> {
    let slice = data.get(p..p + 2).ok_or(ZimError::TruncatedHeader)?;
    if slice.len() != 2 {
        return Err(ZimError::OutOfBounds {
            offset: p as u32,
            len: data.len() as u32,
        });
    }
    Ok(u16::from_le_bytes([slice[0], slice[1]]))
}

/// Read a little-endian u32 at `p`, bounds-checked.
fn read_u32(data: &[u8], p: usize) -> Result<u32, ZimError> {
    let slice = data.get(p..p + 4).ok_or(ZimError::TruncatedHeader)?;
    if slice.len() != 4 {
        return Err(ZimError::OutOfBounds {
            offset: p as u32,
            len: data.len() as u32,
        });
    }
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::header::ZimHeader;

    /// Build a dirent buffer at `off` matching the classic layout, with an
    /// optional truncation (drop the trailing `n` bytes).
    fn build_dirent(data: &mut Vec<u8>, off: usize, truncate: Option<usize>) {
        if data.len() < off {
            data.resize(off, 0u8);
        }
        data.extend_from_slice(b"A/0001_article\x00"); // url + null
        data.extend_from_slice(b"Art one\x00"); // title + null
        data.push(b'A'); // namespace
        data.extend_from_slice(&0u16.to_le_bytes()); // mime_index
        data.extend_from_slice(&2u32.to_le_bytes()); // cluster_index
        data.extend_from_slice(&7u32.to_le_bytes()); // blob_offset
        data.extend_from_slice(&12u32.to_le_bytes()); // blob_size
                                                      // Bytes before `off` are zero-filled by the resize above.
        if let Some(n) = truncate {
            data.truncate(data.len() - n);
        }
    }

    #[test]
    fn dirent_parses_classic_layout() {
        let mut data = Vec::new();
        build_dirent(&mut data, 16, None);
        let d = Dirent::parse(&data, 16).expect("dirent");
        assert_eq!(d.url, "A/0001_article");
        assert_eq!(d.title, "Art one");
        assert_eq!(d.namespace, 'A');
        assert_eq!(d.mime_index, 0);
        assert_eq!(d.cluster_index, 2);
        assert_eq!(d.blob_offset_in_cluster, 7);
        assert_eq!(d.blob_size, 12);
    }

    #[test]
    fn dirent_bad_magic_via_header() {
        // Header magic check gates entry resolution end-to-end.
        let mut data = vec![0u8; 200];
        data[0] = b'X'; // corrupt magic
        let h = ZimHeader::parse(&data);
        assert!(matches!(h, Err(ZimError::InvalidMagic)));
    }

    #[test]
    fn dirent_truncated_errors() {
        let mut data = Vec::new();
        build_dirent(&mut data, 16, Some(3)); // drop last 3 bytes (blob_size)
        match Dirent::parse(&data, 16) {
            Err(ZimError::TruncatedHeader | ZimError::OutOfBounds { offset: _, len: _ }) => {}
            other => panic!("expected truncation error, got {:?}", other),
        }
    }

    #[test]
    fn dirent_out_of_bounds_rejected() {
        let mut data = Vec::new();
        build_dirent(&mut data, 0, Some(6)); // a u32 field runs past EOF
        match Dirent::parse(&data, 0) {
            Err(ZimError::TruncatedHeader | ZimError::OutOfBounds { offset: _, len: _ }) => {}
            other => panic!("expected OutOfBounds, got {:?}", other),
        }
    }
}
