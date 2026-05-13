# stream-to-agora

Stream a local file (and later https/rtmp/rtsp) to an Agora RTC channel as a regular publisher. Ffmpeg decodes the source; raw YUV (video) and PCM (audio) frames are pushed to Agora via the RTC SDK's external-source APIs.

## Status

**Phase 1: connects.** The CLI joins an Agora RTC channel with the
supplied token, prints `ready`, and idles until Ctrl-C (or `--duration`).
No media is streamed yet — that's Phase 2.

| Phase | Milestone | Status |
|---|---|---|
| 0 | CLI surface, arg validation | ✅ |
| 1 | Agora SDK loads, joins channel, logs "ready", idles | ✅ |
| 2 | Stream a static H.264 + AAC file end-to-end | ⏳ next |
| 3 | Arbitrary file via ffmpeg pipeline (any codec ffmpeg decodes) | ⏳ |
| 4 | Remote sources: `https://`, `rtmp://`, `rtsp://` | ⏳ |

## Platforms

Linux (x86_64, aarch64) and macOS (x86_64, aarch64). Windows is not on the roadmap; PRs welcome.

## Install

```bash
npm install -g @agora-build/stream-to-agora
```

Or via shell script:

```bash
curl -fsSL https://dl.agora.build/stream-to-agora/install.sh | bash
```

Or download a binary from [Releases](https://github.com/Agora-Build/stream-to-agora/releases).

(Both packaged installs drop the binary on your `$PATH` and the Agora SDK shared libs in a sibling `lib/` — the binary's rpath finds them at runtime, no `LD_LIBRARY_PATH` needed.)

## Build from source

```bash
git clone git@github.com:Agora-Build/stream-to-agora.git
cd stream-to-agora
# bindgen needs libclang at build time:
#   Debian/Ubuntu:  sudo apt-get install -y libclang-dev
#   macOS:          ships with Xcode command-line tools
cargo build --release         # CMake fetches the Agora SDK on first build
# Binary at target/release/stream-to-agora
```

## Tokens

stream-to-agora **does not mint tokens.** Token minting is a security-sensitive concern that belongs in your token service or in atem; this tool's job is to decode and push frames. The caller supplies a pre-minted RTC token via `--token`.

```bash
TOKEN=$(atem token --channel demo --rtc-user-id 42)
stream-to-agora ./demo.mp4 \
  --app-id      $AGORA_APP_ID \
  --channel     demo \
  --rtc-user-id 42 \
  --token       "$TOKEN"
```

## Usage (planned)

```bash
# Local file (after Phase 3)
stream-to-agora ./demo.mp4 --app-id $AGORA_APP_ID --channel demo --rtc-user-id 42 --token "$TOKEN"

# String account (matches atem's "s/" convention)
stream-to-agora ./demo.mp4 --app-id ... --channel demo --rtc-user-id s/alice --token "$TOKEN"

# Loop forever for steady-state load testing
stream-to-agora ./loop.mp4 --app-id ... --channel demo --rtc-user-id 42 --token "$TOKEN" --loop

# Remote source (Phase 4)
stream-to-agora rtmp://live.example.com/app/key --app-id ... --channel demo --rtc-user-id 42 --token "$TOKEN"
```

## Configuration consistency with atem

Same env-var name and flag conventions so a shell that has atem set up also has stream-to-agora set up:

| Setting | Source | Used by |
|---|---|---|
| App ID | `--app-id` flag or `AGORA_APP_ID` env | atem, stream-to-agora |
| Channel | `--channel` flag | atem, stream-to-agora |
| RTC user | `--rtc-user-id` (with `s/` prefix to force string account) | atem, stream-to-agora |

stream-to-agora does NOT read atem's encrypted credentials store or active project — it's intentionally standalone so it can be dropped on a fresh machine without atem installed.

## Architecture

```
┌─────────────┐   raw YUV    ┌──────────────┐   conn_connect / send_video   ┌────────┐
│   ffmpeg    │ ───────────→ │ stream-to-   │ ───────────────────────────→ │ Agora  │
│  (decoder)  │   raw PCM    │ agora (Rust) │   send_audio_pcm              │  RTC   │
└─────────────┘ ───────────→ │              │ ───────────────────────────→ └────────┘
                              └──────┬───────┘
                                     │ FFI (extern "C")
                                     ▼
                            libagora_rtc_sdk.so   ← Agora NG SDK's flat C API
                                                    (include/c/api2/…)
```

The Agora NG SDK ships a flat C API (`agora_service_create`, `agora_rtc_conn_connect`, `agora_video_frame_sender_send`, …), so Rust links it directly via `extern "C"` — no C++ shim.

- `src/main.rs` — CLI, ffmpeg subprocess management, frame pacing
- `src/agora.rs` — safe Rust wrappers over the SDK's C API *(Phase 1)*
- `CMakeLists.txt` — downloads + stages the Agora SDK at build time, emits include/lib paths
- `build.rs` — runs CMake, links `libagora_rtc_sdk`, sets rpath

## Development

```bash
cargo build              # debug — CMake fetches the SDK on first build
cargo test               # unit tests (CLI parsing, input classification)
cargo clippy             # lint
cargo run -- ./demo.mp4 --app-id $AGORA_APP_ID --channel test --rtc-user-id 42 --token "$TOKEN"
```

Use a pre-staged SDK instead of the auto-download:

```bash
AGORA_RTC_SDK_PATH=/path/to/agora_rtc_sdk cargo build
```

## License

MIT.
