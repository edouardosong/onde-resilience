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
            self.model_id.split('/').next_back().unwrap_or("model"),
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
    /// Repeat penalty (CTRL paper) applied over the last
    /// [`REPEAT_PENALTY_WINDOW`] tokens (prompt tail + generated so far)
    /// before the shaping samplers. 1.0 disables it.
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

/// Maximum number of recent tokens (prompt tail + generated so far) covered
/// by the repeat penalty in [`LlamaContext::generate`].
pub const REPEAT_PENALTY_WINDOW: usize = 64;

/// Recent-token slice covered by the repeat penalty (bounded window).
pub fn penalty_window(tokens: &[i32], window: usize) -> &[i32] {
    let start = tokens.len().saturating_sub(window);
    &tokens[start..]
}

/// Validate accumulated detokenizer byte pieces as UTF-8 text.
///
/// llama.cpp may emit byte-fallback pieces that split a multibyte UTF-8
/// character across several tokens, so pieces cannot be validated one by one.
/// The full byte stream is therefore checked once here: no panics, no
/// unchecked conversion. Returns a clean error for invalid UTF-8.
pub fn decode_token_pieces(bytes: &[u8]) -> Result<String, String> {
    String::from_utf8(bytes.to_vec())
        .map_err(|e| format!("generated text contains invalid UTF-8: {}", e.utf8_error()))
}

/// Byte-level search for stop sequences in not-yet-decoded token pieces.
///
/// Works on raw bytes because the accumulated pieces may end in the middle of
/// a multibyte character. Empty stop tokens are ignored.
pub fn contains_stop_sequence(bytes: &[u8], stop_tokens: &[String]) -> bool {
    stop_tokens
        .iter()
        .filter(|s| !s.is_empty() && s.len() <= bytes.len())
        .any(|s| bytes.windows(s.len()).any(|w| w == s.as_bytes()))
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
    /// Raw llama.cpp pointers (null until `load` succeeds) — only with the `llama-cpp` feature.
    #[cfg(feature = "llama-cpp")]
    model_ptr: *mut llama_cpp_sys::llama_model,
    #[cfg(feature = "llama-cpp")]
    ctx_ptr: *mut llama_cpp_sys::llama_context,
    pub model: GGUFModel,
    pub config: GenerationConfig,
    pub loaded: bool,
}

impl LlamaContext {
    /// Create a new context
    pub fn new(model: GGUFModel, config: GenerationConfig) -> Self {
        tracing::info!("Creating LlamaContext for {:?}", model.model_id);
        Self {
            #[cfg(feature = "llama-cpp")]
            model_ptr: std::ptr::null_mut(),
            #[cfg(feature = "llama-cpp")]
            ctx_ptr: std::ptr::null_mut(),
            model,
            config,
            loaded: false,
        }
    }

    /// Load model from path (GGUF file).
    ///
    /// With the `llama-cpp` feature this performs a real llama.cpp load with a
    /// bounded context size (`n_ctx = 512`) to keep RAM usage limited on-device.
    pub fn load(&mut self, model_path: &str) -> Result<(), String> {
        #[cfg(feature = "llama-cpp")]
        {
            use std::sync::Once;
            static ONCE: Once = Once::new();
            ONCE.call_once(|| unsafe { llama_cpp_sys::llama_backend_init() });

            let c_path = std::ffi::CString::new(model_path)
                .map_err(|_| "Invalid model path (embedded NUL byte)".to_string())?;
            let mp = unsafe { llama_cpp_sys::llama_model_default_params() };
            let m = unsafe { llama_cpp_sys::llama_load_model_from_file(c_path.as_ptr(), mp) };
            if m.is_null() {
                return Err(format!("Failed to load GGUF model from: {}", model_path));
            }

            // Bounded context (512 tokens) → bounded RAM usage.
            let mut cp = unsafe { llama_cpp_sys::llama_context_default_params() };
            cp.n_ctx = 512;
            cp.n_threads = std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4) as u32;
            let c = unsafe { llama_cpp_sys::llama_new_context_with_model(m, cp) };
            if c.is_null() {
                unsafe { llama_cpp_sys::llama_free_model(m) };
                return Err(format!(
                    "Failed to create llama context for: {}",
                    model_path
                ));
            }

            self.model_ptr = m;
            self.ctx_ptr = c;
            self.loaded = true;
            Ok(())
        }

        #[cfg(not(feature = "llama-cpp"))]
        {
            tracing::warn!("Using MOCK llama.cpp context for model: {}", model_path);
            self.loaded = true;
            Ok(())
        }
    }

    /// Generate completion for a prompt.
    ///
    /// With the `llama-cpp` feature this runs the real llama.cpp pipeline:
    /// tokenize → decode → sampling loop (until `max_tokens`, EOS or stop).
    pub async fn generate(&self, prompt: &str) -> Result<GenerationResult, String> {
        if !self.loaded {
            return Err("Model not loaded.".to_string());
        }

        #[cfg(feature = "llama-cpp")]
        {
            self.real_generate(prompt)
        }

        #[cfg(not(feature = "llama-cpp"))]
        {
            self.mock_generate(prompt)
        }
    }

    /// Real llama.cpp inference: tokenize → decode → sampling loop.
    #[cfg(feature = "llama-cpp")]
    fn real_generate(&self, prompt: &str) -> Result<GenerationResult, String> {
        let start = std::time::Instant::now();
        let c_prompt = std::ffi::CString::new(prompt)
            .map_err(|_| "Prompt contains an embedded NUL byte".to_string())?;

        // 1. Tokenize the prompt (with BOS).
        let n_ctx = unsafe { llama_cpp_sys::llama_n_ctx(self.ctx_ptr) } as usize;
        let mut prompt_tokens: Vec<llama_cpp_sys::llama_token> = vec![0; n_ctx];
        let n_prompt = unsafe {
            llama_cpp_sys::llama_tokenize(
                self.model_ptr,
                c_prompt.as_ptr(),
                c_prompt.as_bytes().len() as i32,
                prompt_tokens.as_mut_ptr(),
                n_ctx as i32,
                true,  // add BOS
                false, // no special-token parsing
            )
        };
        if n_prompt < 0 {
            return Err("Prompt too long for the context window".to_string());
        }

        // 2. Decode the prompt (logits only on the last token).
        // NOTE: this llama.cpp version does not set `n_tokens` in init — the caller must.
        let mut batch = unsafe { llama_cpp_sys::llama_batch_init(n_prompt, 0, 1) };
        batch.n_tokens = n_prompt;
        unsafe {
            for i in 0..n_prompt {
                *batch.token.add(i as usize) = prompt_tokens[i as usize];
                *batch.pos.add(i as usize) = i;
                *batch.n_seq_id.add(i as usize) = 1;
                // seq_id[i][0] = 0 (single main sequence)
                let seq_buf_i = *batch.seq_id.add(i as usize);
                *seq_buf_i = 0;
                *batch.logits.add(i as usize) = (i == n_prompt - 1) as i8;
            }
        }
        let rc = unsafe { llama_cpp_sys::llama_decode(self.ctx_ptr, batch) };
        unsafe { llama_cpp_sys::llama_batch_free(batch) };
        if rc != 0 {
            return Err(format!("llama_decode failed (prompt) with code {}", rc));
        }

        // 3. Sampling loop: until max_tokens, EOS or a stop sequence.
        let n_vocab = unsafe { llama_cpp_sys::llama_n_vocab(self.model_ptr) };
        let eos = unsafe { llama_cpp_sys::llama_token_eos(self.model_ptr) };
        // Accumulated raw pieces; validated once at the end by
        // [`decode_token_pieces`] (checked — no from_utf8_unchecked).
        let mut text_bytes: Vec<u8> = Vec::new();
        // Recent tokens for the repeat penalty: prompt tokens, then each
        // generated token as it is sampled.
        let mut recent: Vec<llama_cpp_sys::llama_token> =
            prompt_tokens[..n_prompt as usize].to_vec();
        let mut n_generated: u32 = 0;

        for _ in 0..self.config.max_tokens {
            // Logits of the last decoded token (prompt batch, then single-token batches).
            let logits_idx = if n_generated == 0 { n_prompt - 1 } else { 0 };
            let logits = unsafe { llama_cpp_sys::llama_get_logits_ith(self.ctx_ptr, logits_idx) };

            // Build the candidate array from raw logits.
            let mut cdata: Vec<llama_cpp_sys::llama_token_data> =
                vec![unsafe { std::mem::zeroed() }; n_vocab as usize];
            unsafe {
                for (i, x) in std::slice::from_raw_parts(logits, n_vocab as usize)
                    .iter()
                    .enumerate()
                {
                    cdata[i].id = i as llama_cpp_sys::llama_token;
                    cdata[i].logit = *x;
                }
            }
            let mut candidates = llama_cpp_sys::llama_token_data_array {
                data: cdata.as_mut_ptr(),
                size: n_vocab as usize,
                sorted: false,
            };

            // Repeat penalty over the recent-token window, before the shaping
            // samplers (upstream llama.cpp simple.cpp ordering).
            if self.config.repeat_penalty != 1.0 {
                let window = penalty_window(&recent, REPEAT_PENALTY_WINDOW);
                unsafe {
                    llama_cpp_sys::llama_sample_repetition_penalties(
                        self.ctx_ptr,
                        &mut candidates,
                        window.as_ptr(),
                        window.len(),
                        self.config.repeat_penalty,
                        0.0, // frequency penalty (not exposed by GenerationConfig)
                        0.0, // presence penalty (not exposed by GenerationConfig)
                    );
                }
            }
            unsafe {
                if self.config.temperature > 0.0 {
                    llama_cpp_sys::llama_sample_temp(
                        self.ctx_ptr,
                        &mut candidates,
                        self.config.temperature,
                    );
                }
                llama_cpp_sys::llama_sample_top_k(
                    self.ctx_ptr,
                    &mut candidates,
                    self.config.top_k as i32,
                    1,
                );
                llama_cpp_sys::llama_sample_top_p(
                    self.ctx_ptr,
                    &mut candidates,
                    self.config.top_p,
                    1,
                );
                llama_cpp_sys::llama_sample_softmax(self.ctx_ptr, &mut candidates);
            }
            let token = unsafe { llama_cpp_sys::llama_sample_token(self.ctx_ptr, &mut candidates) };

            if token == eos {
                break;
            }
            recent.push(token);

            // Detokenize (UTF-8).
            let mut buf = [0u8; 512];
            let n_piece = unsafe {
                llama_cpp_sys::llama_token_to_piece(
                    self.model_ptr,
                    token,
                    buf.as_mut_ptr().cast::<std::os::raw::c_char>(),
                    buf.len() as i32,
                )
            };
            if n_piece < 0 {
                return Err("Failed to detokenize a generated token (invalid UTF-8)".to_string());
            }
            text_bytes.extend_from_slice(&buf[..n_piece as usize]);

            // Stop sequences (byte-level: pieces may end mid-character).
            if contains_stop_sequence(&text_bytes, &self.config.stop_tokens) {
                break;
            }

            n_generated += 1;

            // Feed the token back to get logits for the next one (position P + k).
            let mut nb = unsafe { llama_cpp_sys::llama_batch_init(1, 0, 1) };
            nb.n_tokens = 1; // this llama.cpp version does not set it in init
            unsafe {
                *nb.token = token;
                *nb.pos = n_prompt + n_generated as i32 - 1; // 1er token généré en position n_prompt (n_generated déjà incrémenté)
                *nb.n_seq_id = 1;
                **nb.seq_id = 0; // seq_id[0][0] = 0
                *nb.logits = 1;
            }
            let rc = unsafe { llama_cpp_sys::llama_decode(self.ctx_ptr, nb) };
            unsafe { llama_cpp_sys::llama_batch_free(nb) };
            if rc != 0 {
                return Err(format!(
                    "llama_decode failed (token {}) with code {}",
                    n_generated, rc
                ));
            }
        }

        // Checked conversion: clean error instead of latent UB on invalid UTF-8.
        let text = decode_token_pieces(&text_bytes)?;

        let gen_time_ms = start.elapsed().as_millis() as u64;
        Ok(GenerationResult {
            text,
            n_tokens: n_generated,
            gen_time_ms,
            tokens_per_sec: if gen_time_ms > 0 {
                (n_generated as f32) * 1000.0 / gen_time_ms as f32
            } else {
                n_generated as f32
            },
            prompt_tokens: n_prompt as u32,
            peak_mem_mb: self.model.ram_mb,
        })
    }

    /// Free the underlying llama.cpp objects.
    #[cfg(feature = "llama-cpp")]
    fn free_ffi(&mut self) {
        if !self.ctx_ptr.is_null() {
            unsafe { llama_cpp_sys::llama_free(self.ctx_ptr) };
            self.ctx_ptr = std::ptr::null_mut();
        }
        if !self.model_ptr.is_null() {
            unsafe { llama_cpp_sys::llama_free_model(self.model_ptr) };
            self.model_ptr = std::ptr::null_mut();
        }
    }

    #[cfg(not(feature = "llama-cpp"))]
    fn mock_generate(&self, prompt: &str) -> Result<GenerationResult, String> {
        tracing::warn!("Using MOCK generation");

        let responses = ["La RCP (Reanimation Cardio-Pulmonaire) consiste a appliquer des compressions thoraciques altern\u{00e9}es avec des insufflations. Pour un adulte : 30 compressions pour 2 insufflations, a une fr\u{00e9}quence de 100-120 compressions par minute. Appeler les secours (15 ou 112) imm\u{00e9}diatement.",
            "En cas d'h\u{00e9}morragie : 1) Allonger la victime 2) Appuyer fortement sur la plaie avec un tissu propre 3) Faire un pansement compressif 4) Alerter les secours (15, 112). Ne jamais retirer le premier pansement compressif.",
            "Le triangle de Pythagore : Dans un triangle rectangle, a\u{00b2} + b\u{00b2} = c\u{00b2}. Le c\u{00f4}t\u{00e9} c est l'hypot\u{00e9}nuse (le plus long c\u{00f4}t\u{00e9}, oppos\u{00e9} \u{00e0} l'angle droit). Exemple pratique : si a=3 et b=4, alors c=5."];

        let idx = prompt.len() % responses.len();
        let text = responses[idx].to_string();
        let n_tokens = text.len() as u32 / 4;

        Ok(GenerationResult {
            text,
            n_tokens,
            gen_time_ms: 200,
            tokens_per_sec: 45.0,
            prompt_tokens: prompt.len() as u32 / 4,
            peak_mem_mb: self.model.ram_mb,
        })
    }
}

#[cfg(feature = "llama-cpp")]
impl Drop for LlamaContext {
    fn drop(&mut self) {
        self.free_ffi();
    }
}

/// Real llama.cpp FFI (only with the `llama-cpp` feature).
///
/// The C symbols are provided by the `llama_cpp_sys` crate, which builds
/// llama.cpp from source and generates the bindings.
#[cfg(feature = "llama-cpp")]
pub mod ffi {
    /// Initialize the llama.cpp backend (idempotent, safe to call multiple times).
    pub fn init() {
        use std::sync::Once;
        static ONCE: Once = Once::new();
        ONCE.call_once(|| unsafe { llama_cpp_sys::llama_backend_init() });
    }
}

/// Mock FFI initialization (default feature, no real llama.cpp).
#[cfg(not(feature = "llama-cpp"))]
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
        let expected_ram: u64 = 650 * 7000 / 1000; // Q4K = 650MB per B params
        assert!(m.ram_mb > expected_ram.saturating_sub(500));
    }

    #[test]
    fn test_smol_model() {
        let m = GGUFModel::smol_360m(Quantization::Q4K);
        assert_eq!(m.params_b, 0.36);
        assert!(m.arch == ModelArch::SmolLM);
    }

    /// Mock path only: with `llama-cpp`, `load` requires a real GGUF file.
    #[cfg(not(feature = "llama-cpp"))]
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
        assert!(
            Quantization::Q4K.ram_per_billion_params() < Quantization::F32.ram_per_billion_params()
        );
        assert_eq!(Quantization::Q2K.ram_per_billion_params(), 450);
    }

    // ---- Debt fix #1: checked detokenization (no UB, no panic) ----

    #[test]
    fn test_decode_token_pieces_ascii_and_multibyte() {
        let acc = b"bo".to_vec();
        assert_eq!(decode_token_pieces(&acc).unwrap(), "bo");

        let acc = "caf\u{e9}".as_bytes().to_vec();
        assert_eq!(decode_token_pieces(&acc).unwrap(), "caf\u{e9}");
    }

    /// A multibyte UTF-8 character cut in the middle by byte-fallback pieces
    /// must still decode once all pieces are accumulated.
    #[test]
    fn test_decode_token_pieces_handles_multibyte_split_across_tokens() {
        let mut acc: Vec<u8> = Vec::new();
        acc.extend_from_slice(&[b'c', 0xC3]); // first half of 'é' (U+00E9)
        acc.extend_from_slice(&[0xA9, b'!']); // second half
        assert_eq!(decode_token_pieces(&acc).unwrap(), "c\u{e9}!");
    }

    /// Invalid UTF-8 must yield a clean error — never a panic or UB.
    #[test]
    fn test_decode_token_pieces_rejects_invalid_utf8_with_clean_error() {
        let bad_inputs: &[&[u8]] = &[&[0xFF], &[0xC3, 0x28], &[0xE4, 0xB8], &[0xF0, 0x9F, 0x98]];
        for bad in bad_inputs {
            let res = decode_token_pieces(bad);
            assert!(res.is_err(), "expected error for bytes {bad:?}");
            let msg = res.unwrap_err();
            assert!(
                msg.contains("invalid UTF-8"),
                "clean message expected, got: {msg}"
            );
        }
    }

    /// Arbitrary byte streams (deterministic pseudo-random) must either decode
    /// to exactly the UTF-8 interpretation of those bytes or return an error —
    /// and never panic.
    #[test]
    fn test_decode_token_pieces_never_panics_on_arbitrary_bytes() {
        let mut state: u64 = 0xDEAD_BEEF;
        for len in 0..512usize {
            let mut bytes = Vec::with_capacity(len);
            for _ in 0..len {
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                bytes.push((state >> 33) as u8);
            }
            match decode_token_pieces(&bytes) {
                // Ok only when the bytes were valid UTF-8: text must round-trip.
                Ok(text) => assert_eq!(text.as_bytes(), bytes.as_slice()),
                Err(msg) => {
                    // Err only when invalid: std must agree it is not valid UTF-8.
                    assert!(
                        std::str::from_utf8(&bytes).is_err(),
                        "decode_token_pieces rejected valid UTF-8: {msg}"
                    );
                    assert!(
                        msg.contains("invalid UTF-8"),
                        "clean message expected, got: {msg}"
                    );
                }
            }
        }
    }

    #[test]
    fn test_contains_stop_sequence_on_raw_bytes() {
        let stops = vec!["<|im_end|>".to_string()];
        let mut acc: Vec<u8> = Vec::new();
        assert!(!contains_stop_sequence(&acc, &stops));

        acc.extend_from_slice("fin ".as_bytes());
        acc.extend_from_slice(&[0xE4, 0xB8]); // incomplete multibyte char mid-stream
        assert!(!contains_stop_sequence(&acc, &stops));

        acc.extend_from_slice(b"<|im_end|>");
        assert!(contains_stop_sequence(&acc, &stops));
    }

    #[test]
    fn test_contains_stop_sequence_ignores_empty_or_oversized_stops() {
        let stops = vec![String::new(), "STOP".to_string()];
        assert!(!contains_stop_sequence(b"STO", &stops));
        assert!(contains_stop_sequence(b"aSTOP", &stops));
    }

    // ---- Debt fix #2: repeat-penalty recent-token window ----

    #[test]
    fn test_penalty_window_empty_and_short_inputs() {
        assert!(penalty_window(&[], REPEAT_PENALTY_WINDOW).is_empty());
        let toks = [1, 2, 3];
        assert_eq!(penalty_window(&toks, REPEAT_PENALTY_WINDOW), &[1, 2, 3]);
    }

    #[test]
    fn test_penalty_window_keeps_last_n_tokens() {
        let toks: Vec<i32> = (0..100).collect();
        let w = penalty_window(&toks, 64);
        assert_eq!(w.len(), 64);
        assert_eq!(w.first(), Some(&36));
        assert_eq!(w.last(), Some(&99));
    }

    #[test]
    fn test_penalty_window_exact_size_returns_all() {
        let toks: Vec<i32> = (0..64).collect();
        assert_eq!(penalty_window(&toks, 64).len(), 64);
        assert_eq!(penalty_window(&toks, 64), toks.as_slice());
    }

    /// Real llama.cpp inference test (only with the `llama-cpp` feature).
    /// Skipped cleanly if the GGUF model file is not present on this machine.
    #[cfg(feature = "llama-cpp")]
    #[tokio::test]
    async fn test_real_llama_cpp_inference() {
        let model_path = "/home/linux/onde-models/qwen2.5-0.5b-instruct-q4_k_m.gguf";
        if !std::path::Path::new(model_path).exists() {
            eprintln!(
                "SKIP: GGUF model not found at {} — real inference test skipped",
                model_path
            );
            return;
        }

        let mut ctx = LlamaContext::new(
            GGUFModel::qwen_0_5b(Quantization::Q4K),
            GenerationConfig {
                max_tokens: 128,
                ..GenerationConfig::default()
            },
        );
        ctx.load(model_path)
            .expect("real llama.cpp model load should succeed");

        let result = ctx
            .generate("Qu'est-ce que la RCP ?")
            .await
            .expect("real generation should succeed");

        println!("--- real inference (Qwen2.5-0.5B Q4_K_M) ---");
        println!("{}", result.text);
        println!(
            "n_tokens={} prompt_tokens={} gen_time_ms={} tokens_per_sec={:.1}",
            result.n_tokens, result.prompt_tokens, result.gen_time_ms, result.tokens_per_sec
        );

        assert!(
            !result.text.trim().is_empty(),
            "generated text must not be empty"
        );
        assert!(result.n_tokens > 0, "at least one token must be generated");
        assert!(
            result.tokens_per_sec > 0.0,
            "tokens_per_sec must be positive"
        );
    }
}
