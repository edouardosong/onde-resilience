//! Minimal, strictly validating PCM WAV reader (pure Rust — no unsafe).
//!
//! Supports exactly what ONDE needs for local STT: RIFF/WAVE containers with
//! uncompressed 16-bit little-endian PCM, mono or stereo, at the 16 kHz rate
//! required by whisper.cpp. Anything else yields a clean
//! [`SttError::InvalidWav`] — never a panic.

use super::error::SttError;

/// Sample rate required by whisper.cpp models.
pub const TARGET_SAMPLE_RATE: u32 = 16_000;

/// Hard cap on accepted input size (defence against absurd allocations).
const MAX_WAV_BYTES: usize = 256 * 1024 * 1024;

/// Decoded mono audio, normalized to `[-1.0, 1.0]` f32 samples at 16 kHz.
#[derive(Debug, Clone, PartialEq)]
pub struct WavAudio {
    /// Mono samples in `[-1.0, 1.0]`.
    pub samples: Vec<f32>,
    /// Always [`TARGET_SAMPLE_RATE`] on success.
    pub sample_rate: u32,
    /// Source channel count before mixdown (1 or 2).
    pub channels: u16,
    /// Duration rounded to whole milliseconds.
    pub duration_ms: u64,
}

fn invalid<T>(msg: impl Into<String>) -> Result<T, SttError> {
    Err(SttError::InvalidWav(msg.into()))
}

/// Read a little-endian `u16` at `pos`, or fail with a truncation message.
fn u16_at(bytes: &[u8], pos: usize, what: &str) -> Result<u16, SttError> {
    bytes
        .get(pos..pos + 2)
        .map(|s| u16::from_le_bytes([s[0], s[1]]))
        .ok_or_else(|| SttError::InvalidWav(format!("truncated header while reading {what}")))
}

/// Read a little-endian `u32` at `pos`, or fail with a truncation message.
fn u32_at(bytes: &[u8], pos: usize, what: &str) -> Result<u32, SttError> {
    bytes
        .get(pos..pos + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or_else(|| SttError::InvalidWav(format!("truncated header while reading {what}")))
}

/// Parse raw WAV bytes into mono f32 samples ready for whisper.cpp.
pub fn parse_wav(bytes: &[u8]) -> Result<WavAudio, SttError> {
    if bytes.len() > MAX_WAV_BYTES {
        return invalid(format!("input too large: {} bytes", bytes.len()));
    }
    if bytes.len() < 12 {
        return invalid("input shorter than a RIFF header");
    }
    if &bytes[0..4] != b"RIFF" {
        return invalid("missing RIFF magic");
    }
    if &bytes[8..12] != b"WAVE" {
        return invalid("missing WAVE form identifier");
    }

    // Walk chunks after the 12-byte RIFF preamble.
    let mut pos = 12usize;
    let mut format_tag = 0u16;
    let mut channels = 0u16;
    let mut sample_rate = 0u32;
    let mut bits_per_sample = 0u16;
    let mut data: Option<&[u8]> = None;
    let mut saw_fmt = false;

    while pos + 8 <= bytes.len() {
        let chunk_id = &bytes[pos..pos + 4];
        // Chunk size is bounded by the actual remaining input: writers that
        // lie about sizes (streaming files) must not cause panics here.
        let declared = u32_at(bytes, pos + 4, "chunk size")? as usize;
        let body_start = pos + 8;
        let available = bytes.len().saturating_sub(body_start);
        if declared > available {
            return invalid("chunk extends past end of input");
        }
        let body = &bytes[body_start..body_start + declared];

        match chunk_id {
            b"fmt " => {
                if body.len() < 16 {
                    return invalid("fmt chunk shorter than 16 bytes");
                }
                format_tag = u16_at(body, 0, "format tag")?;
                channels = u16_at(body, 2, "channel count")?;
                sample_rate = u32_at(body, 4, "sample rate")?;
                bits_per_sample = u16_at(body, 14, "bits per sample")?;
                saw_fmt = true;
            }
            b"data" => data = Some(body),
            _ => {} // LIST, fact, … skipped
        }
        // Chunks are word-aligned.
        pos = body_start + declared + (declared & 1);
    }

    if !saw_fmt {
        return invalid("missing fmt chunk");
    }
    let pcm = data.ok_or_else(|| SttError::InvalidWav("missing data chunk".into()))?;

    if format_tag != 1 {
        return invalid(format!(
            "unsupported format tag {format_tag} (only uncompressed PCM=1 is supported)"
        ));
    }
    if bits_per_sample != 16 {
        return invalid(format!(
            "unsupported bit depth {bits_per_sample} (16-bit PCM required)"
        ));
    }
    if channels != 1 && channels != 2 {
        return invalid(format!(
            "unsupported channel count {channels} (mono or stereo only)"
        ));
    }
    if sample_rate != TARGET_SAMPLE_RATE {
        return invalid(format!(
            "unsupported sample rate {sample_rate} Hz ({TARGET_SAMPLE_RATE} Hz required)"
        ));
    }
    if pcm.is_empty() || pcm.len() % 2 != 0 {
        return invalid("data chunk is empty or not 16-bit aligned");
    }

    let frames = pcm.len() / 2 / channels as usize;
    if frames == 0 {
        return invalid("no audio frames in data chunk");
    }

    // i16 -> f32 in [-1, 1]; stereo is mixed down to mono by averaging.
    let mut samples = Vec::with_capacity(frames);
    let ch = channels as usize;
    for frame in 0..frames {
        let base = frame * 2 * ch;
        let mut acc = 0f32;
        for c in 0..ch {
            let off = base + c * 2;
            let s = i16::from_le_bytes([pcm[off], pcm[off + 1]]);
            acc += s as f32 / 32768.0;
        }
        samples.push(acc / ch as f32);
    }

    Ok(WavAudio {
        duration_ms: (frames as u64 * 1000) / TARGET_SAMPLE_RATE as u64,
        samples,
        sample_rate,
        channels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal valid WAV byte buffer (PCM16, mono or stereo).
    pub(crate) fn wav_bytes(samples_i16: &[i16], rate: u32, channels: u16) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        let data_len = (samples_i16.len() * 2) as u32;
        let riff_len = 36 + data_len;
        out.extend_from_slice(&riff_len.to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes()); // PCM
        out.extend_from_slice(&channels.to_le_bytes());
        out.extend_from_slice(&rate.to_le_bytes());
        let byte_rate = rate * channels as u32 * 2;
        out.extend_from_slice(&byte_rate.to_le_bytes());
        out.extend_from_slice(&(channels * 2).to_le_bytes()); // block align
        out.extend_from_slice(&16u16.to_le_bytes()); // bits
        out.extend_from_slice(b"data");
        out.extend_from_slice(&data_len.to_le_bytes());
        for s in samples_i16 {
            out.extend_from_slice(&s.to_le_bytes());
        }
        out
    }

    #[test]
    fn parses_valid_mono_16k() {
        let wav = wav_bytes(&[0, 16384, -16384, 32767], TARGET_SAMPLE_RATE, 1);
        let audio = parse_wav(&wav).expect("valid mono wav must parse");
        assert_eq!(audio.channels, 1);
        assert_eq!(audio.sample_rate, 16_000);
        assert_eq!(audio.samples.len(), 4);
        assert!((audio.samples[1] - 0.5).abs() < 1e-6);
        assert!((audio.samples[3] - 0.99997).abs() < 1e-4);
    }

    #[test]
    fn mixes_stereo_down_to_mono() {
        let wav = wav_bytes(&[16384, -16384], TARGET_SAMPLE_RATE, 2);
        let audio = parse_wav(&wav).expect("valid stereo wav must parse");
        assert_eq!(audio.samples.len(), 1); // one frame after mixdown
        assert!(audio.samples[0].abs() < 1e-6); // L+R cancel
    }

    #[test]
    fn rejects_bad_magic_and_short_input() {
        assert!(parse_wav(b"NOTZ").is_err());
        let mut wav = wav_bytes(&[0; 8], TARGET_SAMPLE_RATE, 1);
        wav[0] = b'X';
        assert!(matches!(parse_wav(&wav), Err(SttError::InvalidWav(_))));
        assert!(matches!(parse_wav(&wav[..6]), Err(SttError::InvalidWav(_))));
    }

    #[test]
    fn rejects_truncated_data_chunk_without_panic() {
        let wav = wav_bytes(&[100; 1000], TARGET_SAMPLE_RATE, 1);
        let cut = &wav[..wav.len() - 40];
        assert!(matches!(parse_wav(cut), Err(SttError::InvalidWav(_))));
    }

    #[test]
    fn rejects_wrong_sample_rate_with_clear_message() {
        let wav = wav_bytes(&[0; 160], 44_100, 1);
        match parse_wav(&wav) {
            Err(SttError::InvalidWav(msg)) => {
                assert!(msg.contains("44100"), "message mentions actual rate: {msg}");
                assert!(
                    msg.contains("16000"),
                    "message mentions required rate: {msg}"
                );
            }
            other => panic!("expected InvalidWav, got {other:?}"),
        }
    }

    #[test]
    fn rejects_non_pcm_and_odd_bit_depths() {
        let mut wav = wav_bytes(&[0; 16], TARGET_SAMPLE_RATE, 1);
        // format tag sits at byte 20
        wav[20] = 3; // IEEE float tag
        assert!(matches!(parse_wav(&wav), Err(SttError::InvalidWav(_))));

        let mut wav = wav_bytes(&[0; 16], TARGET_SAMPLE_RATE, 1);
        wav[34] = 8; // 8-bit
        assert!(matches!(parse_wav(&wav), Err(SttError::InvalidWav(_))));
    }
}
