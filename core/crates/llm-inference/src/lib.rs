//! LLM Inference Engine — PocketPal (mobile) + Super-Oracle (desktop)
//!
//! Interface for local AI inference using quantized LLM models.
//! Mobile: Qwen 0.8B-9B (Q4_K_M)
//! Desktop: 70B+ models for oracle API service

use serde::{Deserialize, Serialize};

/// Available model sizes (quantized)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ModelSize {
    Qwen0_8B,  // ~500MB Q4_K_M
    Qwen1_8B,  // ~1.1GB
    Qwen4B,    // ~2.5GB
    Qwen9B,    // ~5.5GB
    Llama70B,  // ~40GB Q4_K_M (desktop oracle)
}

impl ModelSize {
    /// Estimated memory requirement in MB (Q4_K_M)
    pub fn ram_mb(&self) -> u64 {
        match self {
            ModelSize::Qwen0_8B => 512,
            ModelSize::Qwen1_8B => 1200,
            ModelSize::Qwen4B => 2560,
            ModelSize::Qwen9B => 5600,
            ModelSize::Llama70B => 40960,
        }
    }

    /// Check if this model fits in available RAM
    pub fn fits_in_ram(&self, available_mb: u64) -> bool {
        // Leave 20% headroom for system
        self.ram_mb() < (available_mb as f64 * 0.8) as u64
    }
}

/// AI inference request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferenceRequest {
    pub id: String,
    pub prompt: String,
    pub max_tokens: u32,
    pub temperature: f32,
    pub model: ModelSize,
    pub priority: u8,
    /// If Some, this is a remote oracle request
    pub from_mobile: Option<String>,
}

/// AI inference response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferenceResponse {
    pub request_id: String,
    pub text: String,
    pub tokens_generated: u32,
    pub latency_ms: u64,
    pub model_used: ModelSize,
    pub error: Option<String>,
}

/// Oracle RPC service for desktop nodes
pub struct OracleRpcServer {
    port: u16,
    loaded_models: Vec<ModelSize>,
}

impl OracleRpcServer {
    pub fn new(port: u16) -> Self {
        Self {
            port,
            loaded_models: vec![ModelSize::Llama70B],
        }
    }

    /// Start accepting oracle queries from mesh network
    pub async fn start(&self) -> Result<(), String> {
        tracing::info!("Oracle RPC server starting on port {}", self.port);
        // In production: bind TCP socket, accept connections
        // Process inference requests from mobile nodes
        Ok(())
    }

    /// Process an inference request
    pub async fn process(&self, req: InferenceRequest) -> InferenceResponse {
        let start = std::time::Instant::now();

        // In production: actual GGML inference
        let response = format!(
            "[Oracle response using {:?}] Processing: {}",
            req.model,
            &req.prompt.chars().take(50).collect::<String>()
        );

        InferenceResponse {
            request_id: req.id,
            text: response,
            tokens_generated: req.max_tokens.min(256),
            latency_ms: start.elapsed().as_millis() as u64,
            model_used: req.model,
            error: None,
        }
    }
}

/// Mobile PocketPal inference engine
pub struct PocketPalEngine {
    available_ram_mb: u64,
    current_model: Option<ModelSize>,
}

impl PocketPalEngine {
    pub fn new(available_ram_mb: u64) -> Self {
        // Auto-select best model for available RAM
        let model = [
            ModelSize::Qwen9B,
            ModelSize::Qwen4B,
            ModelSize::Qwen1_8B,
            ModelSize::Qwen0_8B,
        ]
        .into_iter()
        .find(|m| m.fits_in_ram(available_ram_mb));

        Self {
            available_ram_mb,
            current_model: model,
        }
    }

    /// Run local inference on mobile device
    pub async fn infer(&self, prompt: &str, max_tokens: u32) -> InferenceResponse {
        let model = self.current_model.clone().unwrap_or(ModelSize::Qwen0_8B);
        let start = std::time::Instant::now();

        // Simulated local inference (production: llama.cpp via FFI)
        tracing::debug!(
            "PocketPal inference with {:?} ({} MB RAM)",
            model,
            self.available_ram_mb
        );

        let response = format!(
            "[PocketPal {:?}] {}",
            model,
            &prompt.chars().take(50).collect::<String>()
        );

        InferenceResponse {
            request_id: format!("local-{}", start.elapsed().as_nanos()),
            text: response,
            tokens_generated: max_tokens.min(128),
            latency_ms: start.elapsed().as_millis() as u64,
            model_used: model,
            error: None,
        }
    }

    pub fn get_model(&self) -> Option<&ModelSize> {
        self.current_model.as_ref()
    }

    pub fn can_offload_to_oracle(&self) -> bool {
        // Mobile can always try to offload complex queries
        self.current_model.as_ref().map(|m| m.ram_mb() < 6000).unwrap_or(true)
    }
}

/// Voice transcription (Speech-to-Text)
pub struct VoiceTranscriber {
    model_size_mb: u32,
}

impl VoiceTranscriber {
    pub fn new() -> Self {
        // Whisper tiny = ~75MB
        Self { model_size_mb: 75 }
    }

    /// Transcribe Opus audio to text
    pub async fn transcribe(&self, _opus_data: &[u8]) -> String {
        // In production: Whisper.cpp via FFI
        // For now, return placeholder
        "[Transcription not available in simulation]".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_ram_check() {
        assert!(ModelSize::Qwen0_8B.fits_in_ram(2048));
        assert!(ModelSize::Qwen4B.fits_in_ram(8192));
        assert!(!ModelSize::Llama70B.fits_in_ram(8192));
        assert!(ModelSize::Llama70B.fits_in_ram(65536));
    }

    #[test]
    fn test_pocket_pal_auto_select() {
        let engine = PocketPalEngine::new(2048);
        assert!(engine.current_model.is_some());
        assert!(engine.can_offload_to_oracle());
    }

    #[tokio::test]
    async fn test_local_inference() {
        let engine = PocketPalEngine::new(4096);
        let resp = engine.infer("Hello world", 50).await;
        assert_ne!(resp.text.len(), 0);
    }
}