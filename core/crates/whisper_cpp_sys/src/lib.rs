//! System-level, **highly** `unsafe` bindings to
//! [whisper.cpp](https://github.com/ggml-org/whisper.cpp).
//!
//! The whisper.cpp sources (stable tag v1.9.3) are vendored under
//! `thirdparty/whisper.cpp/` and compiled as static libraries by `build.rs`
//! (CMake, CPU backend only — no GPU/OpenMP dependencies).
//!
//! C FFI bindings are generated at build time with [`bindgen`] against the
//! vendored `include/whisper.h`. Building this crate therefore requires a
//! C/C++ toolchain, CMake and `libclang`. When libclang needs the GCC builtin
//! headers explicitly, set:
//!
//! ```text
//! BINDGEN_EXTRA_CLANG_ARGS="-isystem /usr/lib/gcc/x86_64-linux-gnu/<v>/include"
//! ```
//!
//! ONDE never depends on this crate directly: it is pulled in only through
//! the optional `whisper-cpp` feature of `whisper-stt` (mock is the default,
//! so CI builds never compile any C code).

#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]

extern crate link_cplusplus;

include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
