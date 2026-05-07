//! Drive the CMake build and stitch the resulting static lib + the
//! Agora RTSA shared lib into the Rust link.
//!
//! Flow:
//!   1. Invoke CMake — it downloads + extracts the Agora RTSA SDK
//!      (or uses the pre-staged path if AGORA_RTC_SDK_PATH env is set)
//!      and compiles `native/src/agora_shim.cpp` into libagora_shim.a.
//!   2. Read `agora_sdk_paths.txt` written by the CMake script to learn
//!      where the SDK's libagora_rtc_sdk.{so,dylib} lives.
//!   3. Emit `cargo:rustc-link-*` directives so the final binary links
//!      both the shim (static) and the SDK (dynamic) and finds the SDK
//!      at runtime via rpath.

use std::env;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=CMakeLists.txt");
    println!("cargo:rerun-if-changed=native/src/agora_shim.cpp");
    println!("cargo:rerun-if-changed=native/include/agora_shim.h");
    println!("cargo:rerun-if-env-changed=AGORA_RTC_SDK_PATH");

    // Forward env override to CMake so callers can `export
    // AGORA_RTC_SDK_PATH=…` and skip the download.
    let mut cfg = cmake::Config::new(".");
    if let Ok(p) = env::var("AGORA_RTC_SDK_PATH") {
        cfg.define("AGORA_RTC_SDK_PATH", p);
    }
    let dst = cfg.build();

    // CMake's `install` step puts artifacts under `${dst}/lib` and the
    // sidecar `agora_sdk_paths.txt` at `${dst}/agora_sdk_paths.txt`.
    let lib_dir = dst.join("lib");
    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    println!("cargo:rustc-link-lib=static=agora_shim");

    let paths_file = dst.join("agora_sdk_paths.txt");
    let (sdk_lib_dir, sdk_lib_name) = read_sdk_paths(&paths_file);
    println!("cargo:rustc-link-search=native={}", sdk_lib_dir);
    println!("cargo:rustc-link-lib=dylib={}", sdk_lib_name);

    // Runtime loader path: find the SDK relative to the binary so a
    // `cargo run` works without LD_LIBRARY_PATH gymnastics. On macOS
    // we use @loader_path; on Linux we use $ORIGIN. The absolute SDK
    // path is also embedded as a fallback for `cargo run` directly.
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    match target_os.as_str() {
        "linux" => {
            println!("cargo:rustc-link-arg=-Wl,-rpath,{}", sdk_lib_dir);
            println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN");
        }
        "macos" => {
            println!("cargo:rustc-link-arg=-Wl,-rpath,{}", sdk_lib_dir);
            println!("cargo:rustc-link-arg=-Wl,-rpath,@loader_path");
        }
        _ => {}
    }

    // C++ stdlib link (required when linking a Rust binary against a
    // static C++ archive that didn't already pull in libstdc++).
    match target_os.as_str() {
        "linux" => println!("cargo:rustc-link-lib=dylib=stdc++"),
        "macos" => println!("cargo:rustc-link-lib=dylib=c++"),
        _ => {}
    }
}

/// Parse the simple KEY=VAL file CMake writes with the resolved SDK
/// paths and pull out (lib_dir, lib_basename_without_lib_prefix).
fn read_sdk_paths(p: &PathBuf) -> (String, String) {
    let txt = std::fs::read_to_string(p)
        .unwrap_or_else(|_| panic!("missing {} — did the CMake build run?", p.display()));
    let mut dir = String::new();
    let mut full_path = String::new();
    for line in txt.lines() {
        if let Some(v) = line.strip_prefix("AGORA_RTC_LIB_DIR=") { dir = v.to_string(); }
        else if let Some(v) = line.strip_prefix("AGORA_RTC_LIB=")  { full_path = v.to_string(); }
    }
    if dir.is_empty() || full_path.is_empty() {
        panic!("malformed agora_sdk_paths.txt:\n{}", txt);
    }
    // Convert /path/to/libagora_rtc_sdk.so → "agora_rtc_sdk"
    let fname = std::path::Path::new(&full_path)
        .file_name().and_then(|s| s.to_str())
        .unwrap_or("");
    let base = fname
        .strip_prefix("lib").unwrap_or(fname)
        .rsplit_once('.').map(|x| x.0).unwrap_or(fname);
    (dir, base.to_string())
}
