# whisper_cpp_sys

Vendored [whisper.cpp](https://github.com/ggml-org/whisper.cpp) C API bindings
for ONDE (raw `unsafe` FFI layer). Architecture mirrors `llama_cpp_sys`
(vendored sources + `build.rs`), as established by T3 (llama-bind).

- Vendored source: whisper.cpp **v1.9.3** (`thirdparty/whisper.cpp/`),
  pruned (no `.git`, examples, tests, samples, docs, media).
- Build: CMake (upstream build system) → static libs `whisper`, `ggml`,
  `ggml-cpu`, `ggml-base`. CPU backend only; `GGML_OPENMP=OFF` (std::thread
  fallback, no libgomp link dependency).
- Bindings: generated at build time by bindgen from `include/whisper.h`.

## Build requirements (only with the `whisper-cpp` feature of `whisper-stt`)

- C/C++ toolchain, CMake, `libclang`.
- If libclang cannot find GCC builtins:
  `BINDGEN_EXTRA_CLANG_ARGS="-isystem /usr/lib/gcc/x86_64-linux-gnu/<v>/include"`

CI never builds this crate: it is excluded from the cargo workspace and pulled
in only through the optional `whisper-cpp` feature (mock is the default).
