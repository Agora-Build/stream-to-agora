// agora_shim.cpp — C++ wrapper around the Agora RTC SDK exposing a
// stable C ABI to Rust.
//
// Phase 0 (this commit): empty translation unit so the file exists.
// Phase 1 will add:
//   - `#include "AgoraBase.h"` and friends
//   - `AgoraEngine` thin object holding IRtcEngine* + observer
//   - external video / audio source push helpers
//   - thread-safe state for the join callback
//
// The build script will compile this file once the SDK is staged.

extern "C" {
// (Phase 1 implementations land here.)
}
