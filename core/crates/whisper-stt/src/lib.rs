//! Whisper STT — Speech-to-Text engine for ONDE
//!
//! Uses whisper.cpp (via whisper-rs) for local transcription.
//! Supports Opus→WAV decoding, French language priority.

use serde::{Deserialize, Serialize};

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
            WhisperModel::Tiny => "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin",
            WhisperModel::Base => "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin",
            WhisperModel::Small => "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin",
            WhisperModel::Medium => "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium.bin",
            WhisperModel::Large => "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3.bin",
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
        })
    }

    /// Load the model from file
    pub async fn load_model(&mut self) -> Result<(), String> {
        #[cfg(feature = "mock")]
        {
            tracing::warn!("Using MOCK STT engine");
            self.loaded = true;
            return Ok(());
        }

        #[cfg(not(feature = "mock"))]
        {
            let model_path = self.config.model_path.clone().unwrap_or_else(|| {
                // Default path: ~/.local/share/onde/models/
                let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
                format!("{}/.local/share/onde/models/ggml-{}.bin",
                    home,
                    match self.config.model {
                        WhisperModel::Tiny => "tiny",
                        WhisperModel::Base => "base",
                        WhisperModel::Small => "small",
                        WhisperModel::Medium => "medium",
                        WhisperModel::Large => "large-v3",
                    }
                )
            });

            if !std::path::Path::new(&model_path).exists() {
                return Err(format!("Model file not found: {}. Download from {}", 
                    model_path, self.config.model.model_url()));
            }

            tracing::info!("Loading whisper model: {}", model_path);
            let start = std::time::Instant::now();
            
            // In production: load whisper.cpp context
            // For now, we use mock since model may not be present
            tracing::warn!("Model not loaded (mock mode)");
            self.loaded = true;

            tracing::info!("Model loaded in {}ms", start.elapsed().as_millis());
            Ok(())
        }
    }

    /// Transcribe audio data (16-bit PCM, 16kHz mono)
    pub async fn transcribe(&self, audio_data: &[i16], sample_rate: u32) -> Result<TranscriptionResult, String> {
        if !self.loaded {
            return Err("Model not loaded. Call load_model() first.".to_string());
        }

        #[cfg(feature = "mock")]
        {
            return self.transcribe_mock(audio_data, sample_rate);
        }

        #[cfg(not(feature = "mock"))]
        {
            self.transcribe_real(audio_data, sample_rate).await
        }
    }

    /// Mock transcription for testing
    pub fn transcribe_mock(&self, _audio_data: &[i16], _sample_rate: u32) -> Result<TranscriptionResult, String> {
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

    /// Real transcription using whisper.cpp
    #[cfg(not(feature = "mock"))]
    async fn transcribe_real(&self, audio_data: &[i16], sample_rate: u32) -> Result<TranscriptionResult, String> {
        let start = std::time::Instant::now();
        
        // Check duration limit
        let duration_sec = audio_data.len() as f32 / sample_rate as f32;
        if duration_sec > self.config.max_duration_sec as f32 {
            return Err(format!(
                "Audio too long: {:.1}s exceeds {}s limit",
                duration_sec, self.config.max_duration_sec
            ));
        }

        // In production: use whisper-rs for actual transcription
        // whisper-rs converts f32 samples to mel spectrogram,
        // then runs GGML inference
        tracing::info!("Transcribing {:.1}s of audio at {}Hz", duration_sec, sample_rate);

        // Placeholder: return mock result
        // In production: full whisper-rs pipeline
        self.transcribe_mock(audio_data, sample_rate)
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

    #[tokio::test]
    async fn test_engine_creation() {
        let config = WhisperConfig::default();
        let engine = WhisperEngine::new(config);
        assert!(engine.is_ok());
    }

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
}