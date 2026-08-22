//! Build script for whisper_cpp_sys.
//!
//! 1. Compiles the vendored whisper.cpp (tag v1.9.3, CPU backend) into static
//!    libraries using CMake — the upstream-supported build system, which
//!    handles the split ggml/ggml-base/ggml-cpu layout automatically.
//! 2. Generates Rust FFI bindings with bindgen against `include/whisper.h`.
//!
//! Mirrors the architecture of `llama_cpp_sys` (vendored sources + build.rs),
//! as established by ONDE T3 (llama-bind).

use std::env;
use std::path::{Path, PathBuf};

const WHISPER_DIR: &str = "thirdparty/whisper.cpp";

fn main() {
    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set"));
    let whisper_dir = manifest_dir.join(WHISPER_DIR);
    let include_dir = whisper_dir.join("include");
    let ggml_include = whisper_dir.join("ggml").join("include");

    for required in [&whisper_dir, &include_dir, &ggml_include] {
        if !required.exists() {
            panic!(
                "whisper_cpp_sys: vendored whisper.cpp is incomplete — '{}' is missing. \
                 The full source tree (tag v1.9.3) must be committed under thirdparty/whisper.cpp/.",
                required.display()
            );
        }
    }
    println!("cargo:rerun-if-changed={}", whisper_dir.display());

    // ------------------------------------------------------------------
    // 1. Static libraries via CMake (CPU backend, no GPU / no OpenMP).
    // ------------------------------------------------------------------
    let dst = cmake::Config::new(&whisper_dir)
        .define("BUILD_SHARED_LIBS", "OFF")
        .define("WHISPER_BUILD_EXAMPLES", "OFF")
        .define("WHISPER_BUILD_TESTS", "OFF")
        .define("WHISPER_BUILD_IS_DEV", "OFF")
        .define("GGML_BUILD_EXAMPLES", "OFF")
        .define("GGML_BUILD_TESTS", "OFF")
        // std::thread fallback inside ggml: avoids a libgomp link dependency
        // and keeps the build identical across environments.
        .define("GGML_OPENMP", "OFF")
        // Keep vendored-code warnings out of the cargo output.
        .define("WHISPER_ALL_WARNINGS", "OFF")
        .define("WHISPER_FATAL_WARNINGS", "OFF")
        .build();

    // CMake installs the archives into <dst>/lib (cmake crate install prefix).
    let lib_dir = dst.join("lib");
    let search_dir = if lib_dir.is_dir() {
        lib_dir
    } else {
        dst.to_path_buf()
    };
    println!("cargo:rustc-link-search=native={}", search_dir.display());
    for lib in link_order(&search_dir) {
        println!("cargo:rustc-link-lib=static={lib}");
    }

    // System libs ggml may reference on Linux (harmless no-ops elsewhere).
    if cfg!(target_os = "linux") {
        println!("cargo:rustc-link-lib=dylib=pthread");
        println!("cargo:rustc-link-lib=dylib=m");
        println!("cargo:rustc-link-lib=dylib=dl");
    }

    // ------------------------------------------------------------------
    // 2. bindgen bindings from the vendored public header.
    // ------------------------------------------------------------------
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR not set"));
    compile_bindings(&out_dir, &include_dir, &ggml_include);
}

/// Dependency-ordered static library names required by ONDE.
///
/// GNU ld resolves archives left to right, so dependents must come before
/// their dependencies: whisper -> ggml -> ggml-cpu -> ggml-base.
/// Other installed archives (e.g. libparakeet.a) are intentionally skipped.
const REQUIRED_LIBS: [&str; 4] = ["whisper", "ggml", "ggml-cpu", "ggml-base"];

fn link_order(search_dir: &Path) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    collect_archives(search_dir, &mut found);

    let ordered: Vec<String> = REQUIRED_LIBS
        .iter()
        .filter(|name| found.iter().any(|f| f == *name))
        .map(|name| name.to_string())
        .collect();
    if ordered.len() < REQUIRED_LIBS.len() {
        let missing: Vec<&str> = REQUIRED_LIBS
            .iter()
            .filter(|n| !found.iter().any(|f| f == *n))
            .copied()
            .collect();
        panic!(
            "whisper_cpp_sys: CMake did not produce expected static libraries \
             {missing:?} under '{}' (found: {found:?})",
            search_dir.display()
        );
    }
    ordered
}

fn collect_archives(dir: &Path, out: &mut Vec<String>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_archives(&path, out);
        } else if path.extension().map(|e| e == "a").unwrap_or(false) {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                let name = stem.strip_prefix("lib").unwrap_or(stem).to_string();
                if !out.contains(&name) {
                    out.push(name);
                }
            }
        }
    }
}

fn compile_bindings(out_dir: &Path, include_dir: &Path, ggml_include: &Path) {
    println!("whisper_cpp_sys: generating bindgen bindings from vendored whisper.h");
    let bindings = bindgen::Builder::default()
        .header(include_dir.join("whisper.h").to_string_lossy())
        .clang_arg(format!("-I{}", include_dir.display()))
        .clang_arg(format!("-I{}", ggml_include.display()))
        .allowlist_function("whisper_.*")
        .allowlist_type("whisper_.*")
        .allowlist_var("WHISPER_.*")
        .default_enum_style(bindgen::EnumVariation::Rust {
            non_exhaustive: true,
        })
        .constified_enum("whisper_gretype")
        .generate_comments(false)
        // NOTE: no derive_partialeq — several param structs contain raw
        // function pointers, which cannot be compared meaningfully.
        .layout_tests(false)
        .generate()
        .expect("whisper_cpp_sys: bindgen failed — is libclang installed? \
                 Hint: BINDGEN_EXTRA_CLANG_ARGS=\"-isystem /usr/lib/gcc/x86_64-linux-gnu/<v>/include\"");
    bindings
        .write_to_file(out_dir.join("bindings.rs"))
        .expect("whisper_cpp_sys: failed to write bindings.rs");
}
