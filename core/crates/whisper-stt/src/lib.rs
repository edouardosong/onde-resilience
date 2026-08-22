//! Whisper STT — Speech-to-Text engine for ONDE
//!
//! Two backends behind cargo features:
//! - `mock` (default): simulated transcription, zero C dependencies — CI-safe;
//! - `whisper-cpp`: real [whisper.cpp](https://github.com/ggml-org/whisper.cpp)
//!   inference through the vendored [`whisper_cpp_sys`] FFI crate
//!   (Phase 2.2 — T8), CPU-only, bounded RAM.
//!
//! Audio input: raw 16-bit PCM samples or strict RIFF/WAVE PCM16 bytes
//! ([`wav::parse_wav`]). Errors are typed (`SttError`) — no panics on bad input.

use serde::{Deserialize, Serialize};

pub mod error;
#[cfg(feature = "whisper-cpp")]
mod real;
pub mod wav;

pub use error::SttError;

/// Speech-to-Text transcription result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptionResult {
    /// Transcribed text
    pub text: String,
    /// Confidence score (0.0 - 1.0)
    pub confidence: f32,
    /// Segments with timestamps
    pub segments: Vec<Segment>,
    /// Language detected
    pub language: String,
    /// Processing time in ms
    pub processing_ms: u64,
}

/// Audio segment with timing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Segment {
    pub text: String,
    pub start_ms: u64,
    pub end_ms: u64,
}

/// Model size for different quality levels
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WhisperModel {
    /// Tiny (75MB) - fastest, lowest quality
    Tiny,
    /// Base (150MB) - balanced mobile
    Base,
    /// Small (500MB) - good quality mobile
    Small,
    /// Medium (1.5GB) - desktop
    Medium,
    /// Large (3GB) — oracle desktop  
    Large,
}

impl WhisperModel {
    /// Estimated memory usage in MB
    pub fn ram_mb(&self) -> u64 {
        match self {
            WhisperModel::Tiny => 256,
            WhisperModel::Base => 512,
            WhisperModel::Small => 1024,
            WhisperModel::Medium => 3072,
            WhisperModel::Large => 5120,
        }
    }

    /// Model file URL (HuggingFace)
    pub fn model_url(&self) -> &'static str {
        match self {
            WhisperModel::Tiny => {
                "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin"
            }
            WhisperModel::Base => {
                "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin"
            }
            WhisperModel::Small => {
                "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin"
            }
            WhisperModel::Medium => {
                "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium.bin"
            }
            WhisperModel::Large => {
                "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3.bin"
            }
        }
    }
}

/// Configuration for STT engine
#[derive(Debug, Clone)]
pub struct WhisperConfig {
    /// Model to use
    pub model: WhisperModel,
    /// Path to model file
    pub model_path: Option<String>,
    /// Target language (auto-detect if None)
    pub language: Option<String>,
    /// Max audio duration in seconds
    pub max_duration_sec: u32,
}

impl Default for WhisperConfig {
    fn default() -> Self {
        Self {
            model: WhisperModel::Tiny,
            model_path: None,
            language: Some("fr".to_string()), // French priority
            max_duration_sec: 120,
        }
    }
}

/// Speech-to-Text engine
pub struct WhisperEngine {
    /// Engine configuration
    pub config: WhisperConfig,
    /// Model loaded flag
    pub loaded: bool,
    /// Raw whisper.cpp context (null-level option until `load_model` succeeds)
    /// — only with the `whisper-cpp` feature.
    #[cfg(feature = "whisper-cpp")]
    ctx: Option<real::RealWhisper>,
}

impl WhisperEngine {
    /// Create a new STT engine
    pub fn new(config: WhisperConfig) -> Result<Self, String> {
        tracing::info!("Initializing WhisperEngine: {:?}", config.model);

        // Check RAM availability
        let available_mb = get_available_ram_mb();
        if config.model.ram_mb() > available_mb {
            return Err(format!(
                "Insufficient RAM: model requires {}MB, only {}MB available",
                config.model.ram_mb(),
                available_mb
            ));
        }

        Ok(Self {
            config,
            loaded: false,
            #[cfg(feature = "whisper-cpp")]
            ctx: None,
        })
    }

    /// Load the model from file.
    ///
    /// Backend precedence when several features are enabled:
    /// `whisper-cpp` (real) > `mock` (default) > clean error if neither.
    pub async fn load_model(&mut self) -> Result<(), String> {
        #[cfg(feature = "whisper-cpp")]
        {
            self.load_model_real()
        }

        #[cfg(all(feature = "mock", not(feature = "whisper-cpp")))]
        {
            tracing::warn!("Using MOCK STT engine");
            self.loaded = true;
            Ok(())
        }

        #[cfg(not(any(feature = "mock", feature = "whisper-cpp")))]
        {
            Err(
                "No STT backend compiled: enable the `mock` (default) or `whisper-cpp` feature."
                    .to_string(),
            )
        }
    }

    /// Default on-disk location for the configured model size.
    #[cfg(feature = "whisper-cpp")]
    fn resolve_model_path(&self) -> String {
        self.config.model_path.clone().unwrap_or_else(|| {
            // Default path: ~/.local/share/onde/models/
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            format!(
                "{}/.local/share/onde/models/ggml-{}.bin",
                home,
                match self.config.model {
                    WhisperModel::Tiny => "tiny",
                    WhisperModel::Base => "base",
                    WhisperModel::Small => "small",
                    WhisperModel::Medium => "medium",
                    WhisperModel::Large => "large-v3",
                }
            )
        })
    }

    /// Real load path: strict existence check, then FFI init via
    /// [`real::RealWhisper`] (CPU-only, RAII-owned context).
    #[cfg(feature = "whisper-cpp")]
    fn load_model_real(&mut self) -> Result<(), String> {
        let model_path = self.resolve_model_path();
        if !std::path::Path::new(&model_path).exists() {
            return Err(SttError::ModelNotFound {
                path: model_path.clone(),
                url: self.config.model.model_url().to_string(),
            }
            .to_string());
        }

        tracing::info!("Loading whisper model (real whisper.cpp): {}", model_path);
        let start = std::time::Instant::now();
        self.ctx = Some(real::RealWhisper::load(&model_path).map_err(|e| e.to_string())?);
        self.loaded = true;
        tracing::info!("Model loaded in {}ms", start.elapsed().as_millis());
        Ok(())
    }

    /// Transcribe audio data (16-bit PCM, 16kHz mono)
    pub async fn transcribe(
        &self,
        audio_data: &[i16],
        sample_rate: u32,
    ) -> Result<TranscriptionResult, String> {
        if !self.loaded {
            return Err("Model not loaded. Call load_model() first.".to_string());
        }

        #[cfg(feature = "whisper-cpp")]
        {
            self.transcribe_real(audio_data, sample_rate)
                .map_err(|e| e.to_string())
        }

        #[cfg(not(feature = "whisper-cpp"))]
        {
            self.transcribe_mock(audio_data, sample_rate)
        }
    }

    /// Transcribe raw WAV bytes (RIFF/WAVE, uncompressed PCM16).
    ///
    /// Strictly validated ([`wav::parse_wav`]); typed errors for invalid or
    /// unsupported input — never a panic. With `whisper-cpp`, runs real
    /// inference; in `mock` builds, validates then answers with mock output.
    pub async fn transcribe_wav(&self, wav_bytes: &[u8]) -> Result<TranscriptionResult, SttError> {
        let audio = wav::parse_wav(wav_bytes)?;

        #[cfg(feature = "whisper-cpp")]
        {
            self.transcribe_f32(&audio.samples)
        }

        #[cfg(not(feature = "whisper-cpp"))]
        {
            tracing::debug!(
                "validated WAV ({} ms, {} ch) — answering with MOCK transcription",
                audio.duration_ms,
                audio.channels
            );
            self.transcribe_mock(&[], wav::TARGET_SAMPLE_RATE)
                .map_err(SttError::Whisper)
        }
    }

    /// Mock transcription for testing
    pub fn transcribe_mock(
        &self,
        _audio_data: &[i16],
        _sample_rate: u32,
    ) -> Result<TranscriptionResult, String> {
        tracing::warn!("Using MOCK transcription");
        let text = "Ceci est une transcription de test du moteur vocal ONDE.";
        Ok(TranscriptionResult {
            text: text.to_string(),
            confidence: 0.92,
            segments: vec![Segment {
                text: text.to_string(),
                start_ms: 0,
                end_ms: 3000,
            }],
            language: "fr".to_string(),
            processing_ms: 150,
        })
    }

    /// Real transcription using whisper.cpp (feature `whisper-cpp`).
    #[cfg(feature = "whisper-cpp")]
    pub fn transcribe_real(
        &self,
        audio_data: &[i16],
        sample_rate: u32,
    ) -> Result<TranscriptionResult, SttError> {
        if sample_rate != wav::TARGET_SAMPLE_RATE {
            return Err(SttError::InvalidWav(format!(
                "unsupported sample rate {sample_rate} Hz ({} Hz required)",
                wav::TARGET_SAMPLE_RATE
            )));
        }

        // Check duration limit
        let duration_sec = audio_data.len() as f32 / sample_rate as f32;
        if duration_sec > self.config.max_duration_sec as f32 {
            return Err(SttError::AudioTooLong {
                duration_sec,
                max_sec: self.config.max_duration_sec,
            });
        }

        tracing::info!(
            "Transcribing (real whisper.cpp) {:.1}s of audio at {}Hz",
            duration_sec,
            sample_rate
        );

        // i16 PCM -> normalized mono f32 expected by whisper_full()
        let samples: Vec<f32> = audio_data.iter().map(|&s| s as f32 / 32768.0).collect();
        self.transcribe_f32(&samples)
    }

    /// Core real pipeline over normalized mono f32 samples at 16 kHz.
    #[cfg(feature = "whisper-cpp")]
    fn transcribe_f32(&self, samples: &[f32]) -> Result<TranscriptionResult, SttError> {
        let ctx = self.ctx.as_ref().ok_or(SttError::NotLoaded)?;
        ctx.transcribe(samples, self.config.language.as_deref())
    }

    /// Get recommended model for available RAM
    pub fn recommend_model(available_ram_mb: u64) -> WhisperModel {
        if available_ram_mb >= 5120 {
            WhisperModel::Large
        } else if available_ram_mb >= 3072 {
            WhisperModel::Medium
        } else if available_ram_mb >= 1024 {
            WhisperModel::Small
        } else if available_ram_mb >= 512 {
            WhisperModel::Base
        } else {
            WhisperModel::Tiny
        }
    }
}

/// Get available system RAM in MB
fn get_available_ram_mb() -> u64 {
    #[cfg(target_os = "android")]
    {
        // Read from /proc/meminfo
        if let Ok(content) = std::fs::read_to_string("/proc/meminfo") {
            for line in content.lines() {
                if line.starts_with("MemAvailable:") {
                    if let Some(val) = line.split_whitespace().nth(1) {
                        if let Ok(kb) = val.parse::<u64>() {
                            return kb / 1024;
                        }
                    }
                }
            }
        }
        2048 // fallback
    }

    #[cfg(not(target_os = "android"))]
    {
        // sysinfo crate in production
        4096 // default
    }
}

/// Download model from URL to path
pub async fn download_model(model: WhisperModel, dest_path: &str) -> Result<String, String> {
    let url = model.model_url();
    tracing::info!("Downloading model {:?} from {}", model, url);

    // In production: use tokio::fs + reqwest for download
    // Show progress bar
    std::fs::create_dir_all(std::path::Path::new(dest_path).parent().unwrap())
        .map_err(|e| format!("Failed to create model dir: {}", e))?;

    tracing::warn!("Model download not implemented in mock mode");
    Ok(dest_path.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    // NOTE: the tests below assert MOCK-engine behaviour; they are compiled
    // only when mock is the *effective* backend (feature `mock` on, real
    // `whisper-cpp` off). Feature combinations are additive in cargo, so
    // enabling `whisper-cpp` routes the engine to the real path (documented
    // precedence: real > mock).
    #[cfg(all(feature = "mock", not(feature = "whisper-cpp")))]
    #[tokio::test]
    async fn test_engine_creation() {
        let config = WhisperConfig::default();
        let engine = WhisperEngine::new(config);
        assert!(engine.is_ok());
    }

    #[cfg(feature = "whisper-cpp")]
    #[tokio::test]
    async fn test_real_engine_creation() {
        let config = WhisperConfig::default();
        let engine = WhisperEngine::new(config);
        assert!(engine.is_ok());
    }

    #[cfg(all(feature = "mock", not(feature = "whisper-cpp")))]
    #[tokio::test]
    async fn test_mock_transcription() {
        let mut engine = WhisperEngine::new(WhisperConfig::default()).unwrap();
        engine.load_model().await.unwrap();

        // 1 second of silence at 16kHz
        let silence = vec![0i16; 16000];
        let result = engine.transcribe(&silence, 16000).await.unwrap();

        assert!(!result.text.is_empty());
        assert!(result.confidence > 0.0);
        assert_eq!(result.language, "fr");
    }

    #[test]
    fn test_model_ram_usage() {
        assert_eq!(WhisperModel::Tiny.ram_mb(), 256);
        assert_eq!(WhisperModel::Large.ram_mb(), 5120);
    }

    #[test]
    fn test_model_recommendation() {
        assert_eq!(WhisperEngine::recommend_model(256), WhisperModel::Tiny);
        assert_eq!(WhisperEngine::recommend_model(2048), WhisperModel::Small);
        assert_eq!(WhisperEngine::recommend_model(8192), WhisperModel::Large);
    }

    /// Build a minimal valid PCM16 WAV buffer (test helper).
    fn wav_bytes(samples_i16: &[i16], rate: u32, channels: u16) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        let data_len = (samples_i16.len() * 2) as u32;
        out.extend_from_slice(&(36 + data_len).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes()); // PCM
        out.extend_from_slice(&channels.to_le_bytes());
        out.extend_from_slice(&rate.to_le_bytes());
        out.extend_from_slice(&(rate * channels as u32 * 2).to_le_bytes());
        out.extend_from_slice(&(channels * 2).to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&data_len.to_le_bytes());
        for s in samples_i16 {
            out.extend_from_slice(&s.to_le_bytes());
        }
        out
    }

    /// 1 s of 440 Hz sine at 16 kHz, rendered as a valid mono WAV.
    fn tone_wav() -> Vec<u8> {
        const RATE: u32 = 16_000;
        let samples: Vec<i16> = (0..RATE as usize)
            .map(|i| {
                let v = (2.0 * std::f32::consts::PI * 440.0 * i as f32 / RATE as f32).sin();
                (v * 8000.0) as i16
            })
            .collect();
        wav_bytes(&samples, RATE, 1)
    }

    #[cfg(all(feature = "mock", not(feature = "whisper-cpp")))]
    #[tokio::test]
    async fn test_transcribe_wav_mock_path_validates_then_answers() {
        let mut engine = WhisperEngine::new(WhisperConfig::default()).unwrap();
        engine.load_model().await.unwrap();
        let result = engine.transcribe_wav(&tone_wav()).await.unwrap();
        assert!(!result.text.is_empty());
        assert_eq!(result.language, "fr"); // mock keeps its documented output
    }

    #[tokio::test]
    async fn test_transcribe_wav_rejects_invalid_magic_cleanly() {
        // Validation precedes any backend work — no load_model needed.
        let engine = WhisperEngine::new(WhisperConfig::default()).unwrap();
        let err = engine.transcribe_wav(b"NOTAWAVFILE!!!!").await.unwrap_err();
        assert!(matches!(err, SttError::InvalidWav(_)), "got {err:?}");
        assert!(err.to_string().contains("invalid WAV"));
    }

    #[tokio::test]
    async fn test_transcribe_wav_rejects_truncated_input_without_panic() {
        let engine = WhisperEngine::new(WhisperConfig::default()).unwrap();
        let wav = tone_wav();
        for cut in [0usize, 13, 30, wav.len() / 2] {
            let res = engine.transcribe_wav(&wav[..cut]).await;
            assert!(res.is_err(), "truncated at {cut} must fail cleanly");
            assert!(matches!(res.unwrap_err(), SttError::InvalidWav(_)));
        }
    }

    #[tokio::test]
    async fn test_transcribe_wav_rejects_unsupported_rate_with_clear_error() {
        let engine = WhisperEngine::new(WhisperConfig::default()).unwrap();
        let wav44 = wav_bytes(&[0i16; 1600], 44_100, 1);
        let err = engine.transcribe_wav(&wav44).await.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("44100") && msg.contains("16000"), "{msg}");
    }

    // ------------------------------------------------------------------
    // Real whisper.cpp path (feature `whisper-cpp`) — T8 Phase 2.2.
    // ------------------------------------------------------------------

    #[cfg(feature = "whisper-cpp")]
    const REAL_MODEL: &str = "/home/linux/onde-models/ggml-tiny.bin";

    #[cfg(feature = "whisper-cpp")]
    #[tokio::test]
    async fn test_real_transcribe_before_load_yields_not_loaded() {
        let engine = WhisperEngine::new(WhisperConfig::default()).unwrap();
        let err = engine.transcribe_wav(&tone_wav()).await.unwrap_err();
        assert!(matches!(err, SttError::NotLoaded), "got {err:?}");
    }

    #[cfg(feature = "whisper-cpp")]
    fn real_model_present() -> bool {
        std::path::Path::new(REAL_MODEL).exists()
    }

    #[cfg(feature = "whisper-cpp")]
    #[tokio::test]
    async fn test_real_missing_model_yields_clean_typed_error() {
        let config = WhisperConfig {
            model_path: Some("/nonexistent/ggml-tiny.bin".into()),
            ..WhisperConfig::default()
        };
        let mut engine = WhisperEngine::new(config).unwrap();
        let err = engine.load_model().await.unwrap_err();
        assert!(err.contains("not found"), "{err}");
        assert!(err.contains("/nonexistent/ggml-tiny.bin"), "{err}");
        assert!(engine.config.model.model_url().contains("huggingface.co"));
    }

    #[cfg(feature = "whisper-cpp")]
    #[tokio::test]
    async fn test_real_incompatible_model_yields_clean_typed_error() {
        if !real_model_present() {
            eprintln!("SKIP: model absent — incompatible-file error still testable without it");
        }
        let tmp = tempfile::Builder::new()
            .suffix(".bin")
            .tempfile()
            .expect("temp file");
        std::fs::write(tmp.path(), b"garbage bytes, definitely not a GGML model").unwrap();

        let config = WhisperConfig {
            model_path: Some(tmp.path().to_string_lossy().into_owned()),
            ..WhisperConfig::default()
        };
        let mut engine = WhisperEngine::new(config).unwrap();
        let err = engine.load_model().await.unwrap_err();
        assert!(err.contains("incompatible"), "{err}");
        assert!(!engine.loaded);
    }

    #[cfg(feature = "whisper-cpp")]
    #[tokio::test]
    async fn test_real_whisper_cpp_transcribes_tone_wav_without_panic() {
        if !real_model_present() {
            eprintln!("SKIP: whisper model not found at {REAL_MODEL} — real STT test skipped");
            return;
        }

        let config = WhisperConfig {
            model_path: Some(REAL_MODEL.into()),
            language: None, // auto-detect — pure tones have no language
            max_duration_sec: 120,
            model: WhisperModel::Tiny,
        };
        let mut engine = WhisperEngine::new(config).unwrap();
        engine.load_model().await.expect("real model load");

        // Scope: the FFI path RUNS and returns a String without panicking.
        // Accuracy on synthetic tones is NOT asserted (follow-up: corpus).
        let result = engine
            .transcribe_wav(&tone_wav())
            .await
            .expect("transcription");
        println!(
            "--- real whisper.cpp on 440 Hz tone (ggml-tiny) ---\n\
             text={:?} lang={} conf={:.3} segments={} ms={}",
            result.text,
            result.language,
            result.confidence,
            result.segments.len(),
            result.processing_ms
        );
        assert!(result.processing_ms > 0 || !result.text.is_empty());
    }

    #[cfg(feature = "whisper-cpp")]
    #[tokio::test]
    async fn test_real_transcribe_i16_silence_returns_ok() {
        if !real_model_present() {
            eprintln!("SKIP: whisper model not found at {REAL_MODEL} — skipped");
            return;
        }
        let config = WhisperConfig {
            model_path: Some(REAL_MODEL.into()),
            language: Some("en".into()),
            model: WhisperModel::Tiny,
            ..WhisperConfig::default()
        };
        let mut engine = WhisperEngine::new(config).unwrap();
        engine.load_model().await.expect("real model load");

        // 1 s of digital silence through the raw-PCM entry point.
        let silence = vec![0i16; 16_000];
        let result = engine
            .transcribe(&silence, 16_000)
            .await
            .expect("ok on silence");
        // Silence may legitimately transcribe to empty text; only the clean
        // Ok(String) contract is asserted here (no panic, typed pipeline).
        let _ = result.text;
        assert!(result.processing_ms > 0 || result.segments.is_empty());
    }

    #[cfg(feature = "whisper-cpp")]
    #[tokio::test]
    async fn test_real_rejects_wrong_sample_rate_before_ffi() {
        if !real_model_present() {
            eprintln!("SKIP: whisper model not found at {REAL_MODEL} — skipped");
            return;
        }
        let config = WhisperConfig {
            model_path: Some(REAL_MODEL.into()),
            ..WhisperConfig::default()
        };
        let mut engine = WhisperEngine::new(config).unwrap();
        engine.load_model().await.unwrap();
        let err = engine.transcribe(&[0i16; 1600], 44_100).await.unwrap_err();
        assert!(err.contains("44100") && err.contains("16000"), "{err}");
    }
}
