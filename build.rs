//! Build script — placeholder for v0.1.
//!
//! Phase 1 (next milestone) responsibilities:
//!   1. Detect target OS (linux | macos) and arch (x86_64 | aarch64).
//!   2. Locate the Agora RTC SDK:
//!      - $AGORA_RTC_SDK_PATH env var if set (manual install)
//!      - else download from a stable URL into `target/agora-sdk/<platform>/`
//!   3. Compile `native/src/agora_shim.cpp` (C++17) against the SDK
//!      headers and link against `libagora_rtc_sdk.{so|dylib}`.
//!   4. Set the runtime loader path:
//!      - linux:  rpath to `$ORIGIN/../lib`
//!      - macos:  install_name_tool / -Wl,-rpath,@loader_path/../Frameworks
//!
//! Phase 0 (this commit): nothing to do — pure Rust binary.

fn main() {
    // Re-run if the C++ shim or this script changes.
    println!("cargo:rerun-if-changed=native/src/agora_shim.cpp");
    println!("cargo:rerun-if-changed=native/include/agora_shim.h");
    println!("cargo:rerun-if-changed=build.rs");
}
