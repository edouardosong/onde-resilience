//! ZIM content extraction — cluster/blob decoding per the classic openZIM spec.
//!
//! A cluster starts with a single compression byte, followed by a sequence of
//! blobs each prefixed by a little-endian u32 size:
//! ```text
//! [compression code]  (1 byte)
//! [u32 size][blob data]   ... repeated until the cluster region ends
//! ```
//! Codes: `'1'` = none, `'4'` = LZ4 block, `'5'` = Zstandard frame.
//!
//! A dirent's `blob_offset_in_cluster` is measured from the start of the
//! cluster (the compression byte itself is offset 0) and points at that
//! blob's u32 size field — so the first blob in a cluster has offset 1.
//!
//! All reads are bounds-checked; decoding yields typed [`ZimError`] variants
//! rather than panicking on malformed input.

use crate::ZimError;

/// Compression codes as stored in a ZIM cluster header byte.
pub const COMP_NONE: u8 = b'1';
pub const COMP_LZ4: u8 = b'4';
pub const COMP_ZSTD: u8 = b'5';

/// A raw (pre-decompression) blob located inside a cluster.
#[derive(Debug, Clone)]
pub struct BlobRaw {
    /// Offset of this blob's size field from the start of the cluster
    /// (the compression byte is offset 0). Comparable with
    /// `Dirent::blob_offset_in_cluster`.
    pub offset_in_cluster: u32,
    /// Compressed payload length in bytes (as declared by the size prefix).
    pub size: u32,
    /// The compressed payload itself.
    pub data: Vec<u8>,
}

impl BlobRaw {
    /// Decompress this blob; `expected_size` is the raw (uncompressed) size
    /// declared by the dirent.
    pub fn decompress(&self, code: u8, expected_size: u32) -> Result<Vec<u8>, ZimError> {
        decompress(code, &self.data, expected_size)
    }
}

/// Parse the blobs of a cluster occupying `data[cluster_start..cluster_end]`.
///
/// Returns the compression code and each located blob. The walk stops at
/// `cluster_end` (typically the start of the next cluster or end of file), so
/// adjacent clusters never bleed into one another's blob tables.
pub fn parse_cluster(
    data: &[u8],
    cluster_start: usize,
    cluster_end: usize,
) -> Result<(u8, Vec<BlobRaw>), ZimError> {
    if cluster_start >= data.len() || cluster_end > data.len() || cluster_start + 1 > cluster_end {
        return Err(ZimError::OutOfBounds {
            offset: cluster_start as u32,
            len: data.len() as u32,
        });
    }
    let code = *data.get(cluster_start).ok_or(ZimError::TruncatedHeader)?;
    // Classic spec: only known compression codes are accepted at the cluster head.
    match code {
        COMP_NONE | COMP_LZ4 | COMP_ZSTD => {}
        other => return Err(ZimError::UnknownCompression(other)),
    }

    // Walk blobs: each is a u32 size header followed by `size` bytes.
    let mut pos = cluster_start + 1;
    let mut blobs = Vec::new();
    while pos < cluster_end {
        if pos + 4 > cluster_end {
            return Err(ZimError::TruncatedHeader); // dangling size prefix
        }
        let size = read_u32(data, pos)?;
        let data_start = pos + 4;
        let data_end = data_start + size as usize;
        if data_end > cluster_end {
            return Err(ZimError::OutOfBounds {
                offset: data_end as u32,
                len: data.len() as u32,
            });
        }
        blobs.push(BlobRaw {
            offset_in_cluster: (pos - cluster_start) as u32,
            size,
            data: data[data_start..data_end].to_vec(),
        });
        pos = data_end;
    }

    Ok((code, blobs))
}

/// Decompress a blob payload that must yield exactly `expected_size` bytes.
pub fn decompress(code: u8, src: &[u8], expected_size: u32) -> Result<Vec<u8>, ZimError> {
    let n = expected_size as usize;
    match code {
        COMP_NONE => {
            if src.len() != n {
                return Err(ZimError::ShortDecompression);
            }
            Ok(src.to_vec())
        }
        COMP_LZ4 => decompress_lz4(src, n),
        COMP_ZSTD => {
            let out = zstd::decode_all(src).map_err(|_| ZimError::ShortDecompression)?;
            if out.len() != n {
                return Err(ZimError::ShortDecompression);
            }
            Ok(out)
        }
        other => Err(ZimError::UnknownCompression(other)),
    }
}

/// LZ4 block decode with exact-length verification.
///
/// `lz4_flex` (with its default `safe-decode` feature) decodes into a caller
/// buffer and reports overflow, but not underflow: a stream shorter than the
/// declared size would leave an uninitialized tail behind. We therefore decode
/// into one spare byte of sentinel value — the decoder writes contiguously from
/// index 0 and never touches the tail — then recover the true decoded length by
/// scanning that sentinel back. Any mismatch with the declared size is a typed
/// error, never silent garbage.
fn decompress_lz4(src: &[u8], expected: usize) -> Result<Vec<u8>, ZimError> {
    let mut buf = vec![0xFFu8; expected + 1];
    lz4_flex::decompress_into(src, &mut buf).map_err(|_| ZimError::ShortDecompression)?;
    let mut true_len = buf.len();
    while true_len > 0 && buf[true_len - 1] == 0xFF {
        true_len -= 1;
    }
    if true_len != expected {
        return Err(ZimError::ShortDecompression);
    }
    Ok(buf[..expected].to_vec())
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

    /// Wrap `payload` into a one-blob cluster with compression `code`.
    fn cluster(code: u8, payload: &[u8]) -> Vec<u8> {
        let mut out = vec![code];
        out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        out.extend_from_slice(payload);
        out
    }

    const HTML: &[u8] =
        b"<html><body><h1>Article</h1><p>Synthetic content for ONDE zim tests.</p></body></html>";

    #[test]
    fn none_roundtrip() {
        let data = cluster(COMP_NONE, HTML);
        let (code, blobs) = parse_cluster(&data, 0, data.len()).expect("cluster");
        assert_eq!(blobs.len(), 1);
        assert_eq!(
            blobs[0].offset_in_cluster, 1,
            "first blob size field sits at offset 1"
        );
        let decoded = decompress(code, &blobs[0].data, HTML.len() as u32).expect("decode");
        assert_eq!(decoded, HTML);
    }

    #[test]
    fn lz4_roundtrip() {
        let payload = lz4_flex::compress(HTML);
        let data = cluster(COMP_LZ4, &payload);
        let (code, blobs) = parse_cluster(&data, 0, data.len()).expect("cluster");
        let decoded = decompress(code, &blobs[0].data, HTML.len() as u32).expect("lz4 decode");
        assert_eq!(decoded, HTML);
    }

    #[test]
    fn zstd_roundtrip() {
        let payload = zstd::bulk::compress(HTML, 1).expect("zstd compress");
        let data = cluster(COMP_ZSTD, &payload);
        let (code, blobs) = parse_cluster(&data, 0, data.len()).expect("cluster");
        let decoded = decompress(code, &blobs[0].data, HTML.len() as u32).expect("zstd decode");
        assert_eq!(decoded, HTML);
    }

    #[test]
    fn multi_blob_offsets_are_cluster_relative() {
        // Two blobs in one cluster: offsets must be relative to the cluster
        // start (code byte = 0), not absolute file positions.
        let a = b"alpha-payload";
        let b = b"beta-payload-longer";
        let mut data = vec![COMP_NONE];
        for p in [a, b] as [&[u8]; 2] {
            data.extend_from_slice(&(p.len() as u32).to_le_bytes());
            data.extend_from_slice(p);
        }
        // Embed the cluster at a non-zero file offset to catch absolute-offset bugs.
        let mut file = vec![0u8; 7];
        file.append(&mut data.clone());
        let start = 7usize;
        let (code, blobs) = parse_cluster(&file, start, file.len()).expect("cluster");
        assert_eq!(blobs.len(), 2);
        assert_eq!(blobs[0].offset_in_cluster, 1);
        let expected_second = 1 + 4 + a.len() as u32;
        assert_eq!(blobs[1].offset_in_cluster, expected_second);
        // Lookup by dirent-style offset must select the right payload.
        for (off, want) in [(1u32, a), (expected_second, b)] as [(u32, &[u8]); 2] {
            let hit = blobs
                .iter()
                .find(|x| x.offset_in_cluster == off)
                .expect("blob found");
            assert_eq!(
                decompress(code, &hit.data, want.len() as u32).unwrap(),
                want
            );
        }
    }

    #[test]
    fn unknown_code_rejected() {
        let data = cluster(b'9', HTML);
        match parse_cluster(&data, 0, data.len()) {
            Err(ZimError::UnknownCompression(b'9')) => {}
            other => panic!("expected UnknownCompression(57), got {:?}", other),
        }
    }

    #[test]
    fn truncated_blob_rejected() {
        let mut data = cluster(COMP_NONE, HTML);
        // Declare 3 more bytes than actually follow: the walk must stop with a
        // typed error instead of reading past `cluster_end`.
        let declared = HTML.len() as u32 + 3;
        data[1..5].copy_from_slice(&declared.to_le_bytes());
        match parse_cluster(&data, 0, data.len()) {
            Err(ZimError::OutOfBounds { .. } | ZimError::TruncatedHeader) => {}
            other => panic!("expected bounds error, got {:?}", other),
        }
    }

    #[test]
    fn lz4_length_mismatch_detected() {
        // Compress a shorter payload but declare the longer expected size:
        // underflow must be a typed error, not an uninitialized tail.
        let short = b"short";
        let payload = lz4_flex::compress(short);
        match decompress(COMP_LZ4, &payload, HTML.len() as u32) {
            Err(ZimError::ShortDecompression) => {}
            other => panic!("expected ShortDecompression, got {:?}", other),
        }
    }

    #[test]
    fn zstd_length_mismatch_detected() {
        let short = b"short";
        let payload = zstd::bulk::compress(short, 1).expect("zstd compress");
        match decompress(COMP_ZSTD, &payload, HTML.len() as u32) {
            Err(ZimError::ShortDecompression) => {}
            other => panic!("expected ShortDecompression, got {:?}", other),
        }
    }

    #[test]
    fn empty_payloads_are_typed_errors_not_panics() {
        for code in [COMP_NONE, COMP_LZ4, COMP_ZSTD] {
            match decompress(code, &[], 5) {
                Err(ZimError::ShortDecompression | ZimError::UnknownCompression(_)) => {}
                other => panic!("code {:?}: expected typed error, got {:?}", code, other),
            }
        }
    }

    #[test]
    fn cluster_bounds_stop_at_next_cluster() {
        // Two adjacent clusters: parsing the first must not walk into the second.
        let mut file = vec![COMP_NONE];
        file.extend_from_slice(&(HTML.len() as u32).to_le_bytes());
        file.extend_from_slice(HTML);
        let end_of_first = file.len();
        // Second cluster with a foreign code byte right after.
        file.push(b'9');
        match parse_cluster(&file, 0, end_of_first) {
            Ok((code, blobs)) => {
                assert_eq!(code, COMP_NONE);
                assert_eq!(blobs.len(), 1);
            }
            other => panic!("expected clean first cluster, got {:?}", other),
        }
    }
}
