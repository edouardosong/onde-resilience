//! llama-bind — GGML/llama bindings for ONDE AI inference
//!
//! Wraps llama.cpp for local LLM inference on resource-constrained devices.
//! Supports Qwen, Phi, TinyLlama and other GGUF-quantized models.

use serde::{Deserialize, Serialize};

/// GGML quantization type
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Quantization {
    /// 2-bit quantization — smallest, lowest quality
    Q2K,
    /// 3-bit quantization
    Q3K,
    /// 4-bit quantization — balanced mobile
    Q4K,
    /// 5-bit quantization
    Q5K,
    /// 6-bit quantization
    Q6K,
    /// 8-bit — higher quality
    Q8_0,
    /// FP16 — desktop
    F16,
    /// FP32 — oracle desktop
    F32,
}

impl Quantization {
    /// Estimated RAM usage in MB for a 1B parameter model
    pub fn ram_per_billion_params(&self) -> u64 {
        match self {
            Quantization::Q2K => 450,
            Quantization::Q3K => 550,
            Quantization::Q4K => 650,
            Quantization::Q5K => 800,
            Quantization::Q6K => 950,
            Quantization::Q8_0 => 1300,
            Quantization::F16 => 2200,
            Quantization::F32 => 4400,
        }
    }

    /// HuggingFace URL template
    pub fn suffix(&self) -> &'static str {
        match self {
            Quantization::Q2K => "q2_k.gguf",
            Quantization::Q3K => "q3_k_m.gguf",
            Quantization::Q4K => "q4_k_m.gguf",
            Quantization::Q5K => "q5_k_m.gguf",
            Quantization::Q6K => "q6_k.gguf",
            Quantization::Q8_0 => "q8_0.gguf",
            Quantization::F16 => "fp16.gguf",
            Quantization::F32 => "fp32.gguf",
        }
    }
}

/// Supported model architecture
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ModelArch {
    /// Qwen 2.5 series (0.5B, 1.5B, 3B, 7B)
    Qwen2_5,
    /// Microsoft Phi-3 mini/medium
    Phi3,
    /// TinyLlama 1.1B
    TinyLlama,
    /// Gemma 2 (2B, 7B)
    Gemma2,
    /// Llama 3.2 (1B, 3B)
    Llama3_2,
    /// SmolLM (135M, 360M)
    SmolLM,
}

/// GGUF model reference
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GGUFModel {
    /// HuggingFace model slug
    pub model_id: String,
    /// Quantization type
    pub quant: Quantization,
    /// Architecture
    pub arch: ModelArch,
    /// Parameter count in billions
    pub params_b: f32,
    /// Estimated RAM for full load
    pub ram_mb: u64,
}

impl GGUFModel {
    /// Recommended Qwen2.5 model for given RAM
    pub fn qwen_for_ram(mb: u64) -> Self {
        if mb >= 5120 {
            GGUFModel::qwen_7b(Quantization::Q4K)
        } else if mb >= 2048 {
            GGUFModel::qwen_3b(Quantization::Q4K)
        } else if mb >= 1024 {
            GGUFModel::qwen_1_5b(Quantization::Q4K)
        } else {
            GGUFModel::qwen_0_5b(Quantization::Q4K)
        }
    }

    pub fn qwen_0_5b(quant: Quantization) -> Self {
        Self {
            model_id: "Qwen/Qwen2.5-0.5B-Instruct-GGUF".to_string(),
            quant,
            arch: ModelArch::Qwen2_5,
            params_b: 0.5,
            ram_mb: quant.ram_per_billion_params() * 500 / 1000,
        }
    }

    pub fn qwen_1_5b(quant: Quantization) -> Self {
        Self {
            model_id: "Qwen/Qwen2.5-1.5B-Instruct-GGUF".to_string(),
            quant,
            arch: ModelArch::Qwen2_5,
            params_b: 1.5,
            ram_mb: quant.ram_per_billion_params() * 1500 / 1000,
        }
    }

    pub fn qwen_3b(quant: Quantization) -> Self {
        Self {
            model_id: "bartowski/Qwen2.5-3B-Instruct-GGUF".to_string(),
            quant,
            arch: ModelArch::Qwen2_5,
            params_b: 3.0,
            ram_mb: quant.ram_per_billion_params() * 3000 / 1000,
        }
    }

    pub fn qwen_7b(quant: Quantization) -> Self {
        Self {
            model_id: "bartowski/Qwen2.5-7B-Instruct-GGUF".to_string(),
            quant,
            arch: ModelArch::Qwen2_5,
            params_b: 7.0,
            ram_mb: quant.ram_per_billion_params() * 7000 / 1000,
        }
    }

    pub fn smol_360m(quant: Quantization) -> Self {
        Self {
            model_id: "HuggingFaceTB/SmolLM-360M-Instruct-GGUF".to_string(),
            quant,
            arch: ModelArch::SmolLM,
            params_b: 0.36,
            ram_mb: quant.ram_per_billion_params() * 360 / 1000,
        }
    }

    /// Get download URL
    pub fn download_url(&self) -> String {
        format!(
            "https://huggingface.co/{}/resolve/main/{}.gguf",
            self.model_id,
            self.model_id.split('/').last().unwrap_or("model"),
        )
    }
}

/// Generation parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationConfig {
    /// Maximum tokens to generate
    pub max_tokens: u32,
    /// Temperature (0.0 = greedy, higher = creative)
    pub temperature: f32,
    /// Top-k sampling
    pub top_k: u32,
    /// Top-p (nucleus) sampling
    pub top_p: f32,
    /// Repeat penalty
    pub repeat_penalty: f32,
    /// Stop sequences
    pub stop_tokens: Vec<String>,
}

impl Default for GenerationConfig {
    fn default() -> Self {
        Self {
            max_tokens: 256,
            temperature: 0.7,
            top_k: 40,
            top_p: 0.9,
            repeat_penalty: 1.1,
            stop_tokens: vec!["<|im_end|>".to_string(), "<|endoftext|>".to_string()],
        }
    }
}

/// Tokenized input
#[derive(Debug, Clone)]
pub struct TokenizedInput {
    pub tokens: Vec<i32>,
    pub n_tokens: usize,
}

/// Generation result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationResult {
    /// Generated text
    pub text: String,
    /// Tokens generated
    pub n_tokens: u32,
    /// Generation time in ms
    pub gen_time_ms: u64,
    /// Tokens per second
    pub tokens_per_sec: f32,
    /// Prompt tokens processed
    pub prompt_tokens: u32,
    /// Memory peak in MB
    pub peak_mem_mb: u64,
}

/// llama.cpp context wrapper
/// In production, this wraps the full llama.cpp context via FFI
pub struct LlamaContext {
    #[cfg(not(feature = "mock"))]
    ctx: Option<std::ptr::NonNull<()>>, // Placeholder for llama_context
    pub model: GGUFModel,
    pub config: GenerationConfig,
    pub loaded: bool,
}

impl LlamaContext {
    /// Create a new context
    pub fn new(model: GGUFModel, config: GenerationConfig) -> Self {
        tracing::info!("Creating LlamaContext for {:?}", model.model_id);
        Self {
            #[cfg(not(feature = "mock"))]
            ctx: None,
            model,
            config,
            loaded: false,
        }
    }

    /// Load model from path
    pub fn load(&mut self, model_path: &str) -> Result<(), String> {
        #[cfg(feature = "mock")]
        {
            tracing::warn!("Using MOCK llama.cpp context for model: {}", model_path);
            self.loaded = true;
            return Ok(());
        }

        #[cfg(not(feature = "mock"))]
        {
            // TODO: Implement real FFI bindings
            // This would:
            // 1. llama_model_load_from_file(model_path, params)
            // 2. llama_new_context(model, ctx_params)
            // For now: mock mode only
            Err("Real llama.cpp bindings not yet implemented. Use 'mock' feature for testing.".to_string())
        }
    }

    /// Generate completion for a prompt
    pub async fn generate(&self, prompt: &str) -> Result<GenerationResult, String> {
        if !self.loaded {
            return Err("Model not loaded.".to_string());
        }

        let start = std::time::Instant::now();

        #[cfg(feature = "mock")]
        {
            return self.mock_generate(prompt);
        }

        #[cfg(not(feature = "mock"))]
        {
            // TODO: Real generation llama.cpp pipeline
            // llama_tokenize -> llama_decode -> llama_sample -> llama_decode (loop)
            Err("Real generation not yet implemented.".to_string())
        }
    }

    fn mock_generate(&self, prompt: &str) -> Result<GenerationResult, String> {
        tracing::warn!("Using MOCK generation");

        let responses = vec![
            "La RCP (Réanimation Cardio-Pulmonaire) consiste à appliquer des compressions thoraciques alternées avec des insufflations. Pour un adulte : 30 compressions pour 2 insufflations, à une fréquence de 100-120 compressions par minute. Appeler les secours (15 ou 112) immédiatement.",
            "En cas d'hémorragie : 1) Allonger la victime 2) Appuyer fortement sur la plaie avec un tissu propre 3) Faire un pansement compressif 4) Alerter les secours (15, 112). Ne jamais retirer le premier pansement compressif.",
            "Le triangle de Pythagore : Dans un triangle rectangle, a² + b² = c². Le côté c est l'hypoténuse (le plus long côté, opposé à l'angle droit). Exemple pratique : si a=3 et b=4, alors c=5.",
        ];

        let idx = prompt.len() % responses.len();
        let text = responses[idx].to_string();

        let gen_time_ms = start.elapsed().as_millis() as u64 + 200;

        Ok(GenerationResult {
            text,
            n_tokens: text.len() as u32 / 4, // Rough estimate
            gen_time_ms,
            tokens_per_sec: 45.0,
            prompt_tokens: prompt.len() as u32 / 4,
            peak_mem_mb: self.model.ram_mb,
        })
    }
}

/// FFI declarations for llama.cpp (in production)
#[cfg(not(feature = "mock"))]
pub mod ffi {
    // These would be auto-generated by bindgen from llama.h
    // Placeholder declarations:
    //
    // #[link(name = "llama")]
    // extern "C" {
    //     pub fn llama_backend_init();
    //     pub fn llama_model_load_from_file(
    //         path: *const std::os::raw::c_char,
    //         params: *mut llama_model_params,
    //     ) -> *mut llama_model;
    //     // ... more declarations
    // }

    pub fn init() {
        tracing::warn!("llama.cpp FFI not available in mock mode");
    }
}

#[cfg(feature = "mock")]
pub fn init_ffi() {
    tracing::warn!("Mock llama.cpp FFI initialized");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_qwen_model_selection() {
        let m = GGUFModel::qwen_for_ram(512);
        assert_eq!(m.params_b, 0.5);
        assert!(m.model_id.contains("0.5B"));
    }

    #[test]
    fn test_qwen_7b() {
        let m = GGUFModel::qwen_7b(Quantization::Q4K);
        assert_eq!(m.params_b, 7.0);
        let expected_ram = 650 * 7000 / 1000; // Q4K = 650MB per B params
        assert!(m.ram_mb > expected_ram.saturating_sub(500));
    }

    #[test]
    fn test_smol_model() {
        let m = GGUFModel::smol_360m(Quantization::Q4K);
        assert_eq!(m.params_b, 0.36);
        assert!(m.arch == ModelArch::SmolLM);
    }

    #[tokio::test]
    async fn test_mock_generation() {
        let model = GGUFModel::qwen_0_5b(Quantization::Q4K);
        let config = GenerationConfig::default();
        let mut ctx = LlamaContext::new(model, config);

        ctx.load("mock_model.gguf").unwrap();

        let result = ctx.generate("Premiers secours?").await.unwrap();
        assert!(!result.text.is_empty());
        assert!(result.n_tokens > 0);
        assert!(result.tokens_per_sec > 0.0);
    }

    #[test]
    fn test_quantization_ram() {
        assert!(Quantization::Q4K.ram_per_billion_params() < Quantization::F32.ram_per_billion_params());
        assert_eq!(Quantization::Q2K.ram_per_billion_params(), 450);
    }
}