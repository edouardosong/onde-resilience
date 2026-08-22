//! Real whisper.cpp transcription path (feature `whisper-cpp`).
//!
//! Owns the raw `whisper_context*`. The only `unsafe` in ONDE lives here and
//! is limited to direct whisper.cpp C API calls; every pointer crossing FFI is
//! checked for null / non-zero status before being dereferenced or converted.
//!
//! SAFETY notes:
//! - `whisper_context` is **not** thread-safe; `RealWhisper` deliberately does
//!   not implement `Send`/`Sync`, so the compiler prevents sharing it across
//!   threads.
//! - The context is released exactly once via `Drop` → `whisper_free`.
//! - The `CString` backing `params.language` outlives the `whisper_full` call.

use std::ffi::{CStr, CString};
use std::time::Instant;

use whisper_cpp_sys as sys;

use super::error::SttError;
use super::TranscriptionResult;

/// RAII handle over a loaded whisper.cpp model + default state.
pub(crate) struct RealWhisper {
    ctx: *mut sys::whisper_context,
}

impl RealWhisper {
    /// Load a GGML model file. CPU-only (`use_gpu = false`) for bounded RAM
    /// and environment-independent behaviour.
    pub fn load(model_path: &str) -> Result<Self, SttError> {
        let c_path = CString::new(model_path)
            .map_err(|_| SttError::Whisper("model path contains a NUL byte".into()))?;

        // SAFETY: c_path is a valid NUL-terminated buffer; params returned by
        // value from whisper_context_default_params().
        let mut ctx_params = unsafe { sys::whisper_context_default_params() };
        ctx_params.use_gpu = false;

        // SAFETY: both arguments are valid for the duration of the call.
        let ctx = unsafe { sys::whisper_init_from_file_with_params(c_path.as_ptr(), ctx_params) };
        if ctx.is_null() {
            return Err(SttError::ModelIncompatible {
                path: model_path.to_string(),
                reason: "whisper_init_from_file_with_params returned null (not a readable                          whisper GGML model)"
                    .into(),
            });
        }
        Ok(Self { ctx })
    }

    /// Run full transcription on mono f32 samples at 16 kHz.
    pub fn transcribe(
        &self,
        samples: &[f32],
        language: Option<&str>,
    ) -> Result<TranscriptionResult, SttError> {
        if samples.is_empty() {
            return Err(SttError::InvalidWav(
                "no audio samples to transcribe".into(),
            ));
        }
        let start = Instant::now();

        // SAFETY: returns params by value for the vendored header version.
        let mut params = unsafe {
            sys::whisper_full_default_params(
                sys::whisper_sampling_strategy::WHISPER_SAMPLING_GREEDY,
            )
        };
        params.n_threads = std::thread::available_parallelism()
            .map(|n| n.get() as i32)
            .unwrap_or(1)
            .max(1);
        params.translate = false;
        params.print_special = false;
        params.print_progress = false;
        params.print_realtime = false;
        params.print_timestamps = false;
        params.single_segment = false;
        params.no_context = true;
        params.suppress_blank = true;
        params.suppress_nst = true;
        params.detect_language = language.is_none();

        // Keep the language buffer alive across whisper_full().
        let lang_c = match language {
            Some(lang) => Some(
                CString::new(lang)
                    .map_err(|_| SttError::Whisper("language tag contains a NUL byte".into()))?,
            ),
            None => None,
        };
        if let Some(l) = &lang_c {
            params.language = l.as_ptr();
        }

        // SAFETY: self.ctx is a live context (non-null since load); samples is
        // a valid f32 slice with len() elements.
        let rc =
            unsafe { sys::whisper_full(self.ctx, params, samples.as_ptr(), samples.len() as i32) };
        if rc != 0 {
            return Err(SttError::Whisper(format!(
                "whisper_full failed with code {rc}"
            )));
        }

        // Collect segments (text + timestamps + mean token probability).
        // SAFETY: all getters take the live context and in-range indices.
        let n_segments = unsafe { sys::whisper_full_n_segments(self.ctx) };
        if n_segments < 0 {
            return Err(SttError::Whisper(format!(
                "whisper_full_n_segments returned {n_segments}"
            )));
        }
        let mut text = String::new();
        let mut segments = Vec::with_capacity(n_segments as usize);
        let mut prob_sum = 0f64;
        let mut prob_count = 0u64;
        for i in 0..n_segments {
            let text_ptr = unsafe { sys::whisper_full_get_segment_text(self.ctx, i) };
            let seg_text = if text_ptr.is_null() {
                String::new()
            } else {
                // SAFETY: whisper.cpp returns a NUL-terminated static buffer.
                unsafe { CStr::from_ptr(text_ptr) }
                    .to_string_lossy()
                    .into_owned()
            };
            let t0 = unsafe { sys::whisper_full_get_segment_t0(self.ctx, i) };
            let t1 = unsafe { sys::whisper_full_get_segment_t1(self.ctx, i) };

            let n_tokens = unsafe { sys::whisper_full_n_tokens(self.ctx, i) };
            for tok in 0..n_tokens.max(0) {
                prob_sum += unsafe { sys::whisper_full_get_token_p(self.ctx, i, tok) } as f64;
                prob_count += 1;
            }

            text.push_str(&seg_text);
            segments.push(super::Segment {
                text: seg_text.trim().to_string(),
                start_ms: (t0.max(0) as u64) / 10, // centiseconds -> ms
                end_ms: (t1.max(0) as u64) / 10,
            });
        }

        let confidence = if prob_count > 0 {
            ((prob_sum / prob_count as f64).clamp(0.0, 1.0)) as f32
        } else {
            0.0
        };

        let lang_id = unsafe { sys::whisper_full_lang_id(self.ctx) };
        let detected = if lang_id >= 0 {
            // SAFETY: returns a static string or null for invalid ids.
            let ptr = unsafe { sys::whisper_lang_str(lang_id) };
            if ptr.is_null() {
                "auto".to_string()
            } else {
                // SAFETY: NUL-terminated static buffer checked above.
                unsafe { CStr::from_ptr(ptr) }
                    .to_string_lossy()
                    .into_owned()
            }
        } else {
            "auto".to_string()
        };

        Ok(TranscriptionResult {
            text,
            confidence,
            segments,
            language: detected,
            processing_ms: u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
        })
    }
}

impl Drop for RealWhisper {
    fn drop(&mut self) {
        // SAFETY: self.ctx was produced by whisper_init_from_file_with_params
        // and is freed exactly once here.
        if !self.ctx.is_null() {
            unsafe { sys::whisper_free(self.ctx) };
            self.ctx = std::ptr::null_mut();
        }
    }
}

// Deliberately NOT Send/Sync: whisper contexts are single-threaded objects.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_rejects_incompatible_file_with_clean_error() {
        let tmp = tempfile::Builder::new()
            .suffix(".bin")
            .tempfile()
            .expect("temp file");
        std::fs::write(tmp.path(), b"definitely not a ggml whisper model").unwrap();

        match RealWhisper::load(tmp.path().to_str().unwrap()) {
            Err(SttError::ModelIncompatible { path, reason }) => {
                assert!(path.ends_with(".bin"));
                assert!(!reason.is_empty());
            }
            Ok(_) => panic!("load should have failed on garbage model"),
            Err(e) => panic!("expected ModelIncompatible, got {e:?}"),
        }
    }
}
