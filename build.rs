//! Drive the CMake SDK-staging step and wire the Agora RTSA shared lib
//! into the Rust link.
//!
//! Flow:
//!   1. Invoke CMake — it downloads + extracts the Agora RTSA SDK
//!      (or uses the pre-staged path if AGORA_RTC_SDK_PATH env is set)
//!      and writes `agora_sdk_paths.txt`. No C++ is compiled: the SDK
//!      exposes a flat C API that Rust calls directly via `extern "C"`.
//!   2. Read `agora_sdk_paths.txt` to learn where the SDK's
//!      libagora_rtc_sdk.{so,dylib} lives.
//!   3. Emit `cargo:rustc-link-*` directives so the final binary links
//!      the SDK (dynamic) and finds it at runtime via rpath.
//!   4. Run bindgen to generate Rust FFI bindings from the flat C API
//!      headers, writing `agora_sys.rs` to `$OUT_DIR`.

use std::env;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=CMakeLists.txt");
    println!("cargo:rerun-if-env-changed=AGORA_RTC_SDK_PATH");

    // Forward env override to CMake so callers can `export
    // AGORA_RTC_SDK_PATH=…` and skip the download.
    let mut cfg = cmake::Config::new(".");
    if let Ok(p) = env::var("AGORA_RTC_SDK_PATH") {
        cfg.define("AGORA_RTC_SDK_PATH", p);
    }
    let dst = cfg.build();

    // CMake installs the `agora_sdk_paths.txt` sidecar at `${dst}/agora_sdk_paths.txt`.
    let paths_file = dst.join("agora_sdk_paths.txt");
    let (sdk_lib_dir, sdk_lib_name, sdk_include_dir) = read_sdk_paths(&paths_file);
    println!("cargo:rustc-link-search=native={}", sdk_lib_dir);
    println!("cargo:rustc-link-lib=dylib={}", sdk_lib_name);

    // Runtime loader path: find the SDK relative to the binary so a
    // `cargo run` works without LD_LIBRARY_PATH gymnastics. On macOS
    // we use @loader_path; on Linux we use $ORIGIN. The absolute SDK
    // path is also embedded as a fallback for `cargo run` directly.
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    match target_os.as_str() {
        "linux" => {
            // Use DT_RPATH (not DT_RUNPATH): libagora_rtc_sdk.so has its own
            // DT_NEEDED siblings (libagora-fdkaac.so, libaosl.so) but no rpath
            // of its own. The loader searches the executable's DT_RPATH for
            // those transitive deps, but NOT DT_RUNPATH. --disable-new-dtags
            // flips the default back to DT_RPATH so sibling SDK libs resolve.
            println!("cargo:rustc-link-arg=-Wl,--disable-new-dtags");
            // Order matters: $ORIGIN-relative paths first (deployed layouts),
            // then the absolute build-tree path as a fallback for `cargo run`
            // straight from target/.
            println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN");                       // co-located (curl-install tarball, dev runs from target/release)
            println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/../lib");                // npm package layout (bin/ + lib/ siblings)
            println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/../lib/stream-to-agora"); // install.sh layout (/usr/local/bin + /usr/local/lib/stream-to-agora)
            println!("cargo:rustc-link-arg=-Wl,-rpath,{}", sdk_lib_dir);
        }
        "macos" => {
            // macOS uses DT_RPATH semantics by default; no --disable-new-dtags needed.
            println!("cargo:rustc-link-arg=-Wl,-rpath,@loader_path");
            println!("cargo:rustc-link-arg=-Wl,-rpath,@loader_path/../lib");
            println!("cargo:rustc-link-arg=-Wl,-rpath,@loader_path/../lib/stream-to-agora");
            println!("cargo:rustc-link-arg=-Wl,-rpath,{}", sdk_lib_dir);
        }
        _ => {}
    }

    // --- Generate Rust bindings for the flat C API via bindgen ---
    println!("cargo:rerun-if-changed=wrapper.h");
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let bindings = bindgen::Builder::default()
        .header("wrapper.h")
        .clang_arg(format!("-I{}", sdk_include_dir))                 // .../agora_sdk/include
        .clang_arg(format!("-I{}/c", sdk_include_dir))
        .clang_arg(format!("-I{}/c/base", sdk_include_dir))
        .clang_arg(format!("-I{}/c/api2", sdk_include_dir))
        .allowlist_function("agora_.*")
        .allowlist_type("agora_.*|_?rtc_conn_.*|_?agora_service_config|user_id_t|conn_id_t|uid_t|AGORA_HANDLE")
        .allowlist_var("AGORA_.*")
        .default_enum_style(bindgen::EnumVariation::Rust { non_exhaustive: false })
        .derive_default(true)
        .layout_tests(false)
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .generate()
        .expect("bindgen failed to generate Agora C API bindings");
    bindings.write_to_file(out_dir.join("agora_sys.rs")).expect("couldn't write agora_sys.rs");
}

/// Parse the simple KEY=VAL file CMake writes with the resolved SDK
/// paths and pull out (lib_dir, lib_basename_without_lib_prefix, include_dir).
fn read_sdk_paths(p: &PathBuf) -> (String, String, String) {
    let txt = std::fs::read_to_string(p)
        .unwrap_or_else(|_| panic!("missing {} — did the CMake build run?", p.display()));
    let mut dir = String::new();
    let mut full_path = String::new();
    let mut include = String::new();
    for line in txt.lines() {
        if let Some(v) = line.strip_prefix("AGORA_RTC_LIB_DIR=") { dir = v.to_string(); }
        else if let Some(v) = line.strip_prefix("AGORA_RTC_LIB=")  { full_path = v.to_string(); }
        else if let Some(v) = line.strip_prefix("AGORA_RTC_INCLUDE_DIR=") { include = v.to_string(); }
    }
    if dir.is_empty() || full_path.is_empty() || include.is_empty() {
        panic!("malformed agora_sdk_paths.txt:\n{}", txt);
    }
    // Convert /path/to/libagora_rtc_sdk.so → "agora_rtc_sdk"
    let fname = std::path::Path::new(&full_path)
        .file_name().and_then(|s| s.to_str())
        .unwrap_or("");
    let base = fname
        .strip_prefix("lib").unwrap_or(fname)
        .rsplit_once('.').map(|x| x.0).unwrap_or(fname);
    (dir, base.to_string(), include)
}
