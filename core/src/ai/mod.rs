/// AI Module — PocketPal integration & Oracle RPC bridge

pub use llm_inference::{
    InferenceRequest,
    InferenceResponse,
    ModelSize,
    OracleRpcServer,
    PocketPalEngine,
    VoiceTranscriber,
};

/// AI Engine manager — decides local vs oracle
pub struct AiEngine {
    local_engine: PocketPalEngine,
    oracle_address: Option<String>,
}

impl AiEngine {
    pub fn new(available_ram_mb: u64) -> Self {
        Self {
            local_engine: PocketPalEngine::new(available_ram_mb),
            oracle_address: None,
        }
    }

    /// Run inference — local or offload to oracle
    pub async fn infer(&self, prompt: &str, max_tokens: u32) -> InferenceResponse {
        // Try local first
        let resp = self.local_engine.infer(prompt, max_tokens).await;

        // If local model is small and prompt is complex, prefer oracle
        if self.local_engine.can_offload_to_oracle() && self.oracle_address.is_some() {
            // In production: send to oracle via RPC
            tracing::debug!("Could offload to oracle: {}", self.oracle_address.as_ref().unwrap());
        }

        resp
    }

    /// Set oracle address for desktop node
    pub fn set_oracle(&mut self, address: String) {
        self.oracle_address = Some(address);
    }

    pub fn get_local_model(&self) -> Option<&ModelSize> {
        self.local_engine.get_model()
    }
}