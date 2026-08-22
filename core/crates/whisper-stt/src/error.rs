//! Typed, clean errors for the STT pipeline (T8 — Phase 2.2).
//!
//! The legacy `Result<_, String>` signatures of [`crate::WhisperEngine`] are
//! kept for compatibility; new APIs return this enum. Every failure mode is
//! recoverable: no panics, no unwraps on external input.

use std::fmt;

/// Errors produced by the speech-to-text engine.
#[derive(Debug)]
pub enum SttError {
    /// Model file does not exist on disk.
    ModelNotFound {
        /// Path that was probed.
        path: String,
        /// Where to download a valid model.
        url: String,
    },
    /// Audio input is not a readable 16 kHz PCM WAV stream.
    InvalidWav(String),
    /// File exists but whisper.cpp cannot use it (corrupted / foreign format).
    ModelIncompatible {
        /// Path of the rejected file.
        path: String,
        /// Why it was rejected.
        reason: String,
    },
    /// Input audio exceeds the configured duration limit.
    AudioTooLong {
        /// Detected duration in seconds.
        duration_sec: f32,
        /// Configured maximum in seconds.
        max_sec: u32,
    },
    /// Engine used before `load_model()` succeeded.
    NotLoaded,
    /// whisper.cpp call failed (non-zero status or null pointer).
    Whisper(String),
}

impl fmt::Display for SttError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ModelNotFound { path, url } => {
                write!(f, "model file not found: {path} (download from {url})")
            }
            Self::InvalidWav(msg) => write!(f, "invalid WAV audio: {msg}"),
            Self::ModelIncompatible { path, reason } => {
                write!(f, "model incompatible: {path} ({reason})")
            }
            Self::AudioTooLong {
                duration_sec,
                max_sec,
            } => write!(
                f,
                "audio too long: {duration_sec:.1}s exceeds {max_sec}s limit"
            ),
            Self::NotLoaded => write!(f, "model not loaded: call load_model() first"),
            Self::Whisper(msg) => write!(f, "whisper.cpp failure: {msg}"),
        }
    }
}

impl std::error::Error for SttError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_mentions_path_and_url_for_missing_model() {
        let err = SttError::ModelNotFound {
            path: "/x/ggml-tiny.bin".into(),
            url: "https://example.invalid/tiny.bin".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("/x/ggml-tiny.bin"), "{msg}");
        assert!(msg.contains("https://example.invalid/tiny.bin"), "{msg}");
        assert!(msg.contains("not found"), "{msg}");
    }

    #[test]
    fn display_reports_wav_problem() {
        assert!(SttError::InvalidWav("bad magic".into())
            .to_string()
            .contains("invalid WAV"));
    }

    #[test]
    fn display_reports_incompatibility() {
        let err = SttError::ModelIncompatible {
            path: "/x/m.bin".into(),
            reason: "not a GGML model".into(),
        };
        let msg = err.to_string();
        assert!(
            msg.contains("incompatible") && msg.contains("/x/m.bin"),
            "{msg}"
        );
    }
}
