# stream-to-agora

Stream a local file (and later https/rtmp/rtsp) to an Agora RTC channel as a regular publisher. Ffmpeg decodes the source; raw YUV (video) and PCM (audio) frames are pushed to Agora via the RTC SDK's external-source APIs.

## Status

**Phase 2: publishes.** The CLI runs ffmpeg against any input file it can
read, then publishes one audio + one video track to the RTC channel.
If the input is H.264 video + AAC audio, the encoded frames pass through
unchanged (`-c copy`, zero-CPU). Any other codec ffmpeg can decode
(H.265 / VP8 / VP9 / AV1 / Opus / MP3 / Vorbis / PCM / …) is decoded to
raw YUV+PCM and pushed via Agora's raw-frame senders; Agora's edge
re-encodes downstream. `--loop` for steady-state publish; `--duration`
for bounded soak runs.

| Phase | Milestone | Status |
|---|---|---|
| 0 | CLI surface, arg validation | ✅ |
| 1 | Agora SDK loads, joins channel, logs "ready", idles | ✅ |
| 2 | Publish a local file via ffmpeg (any codec ffmpeg reads) | ✅ |
| 3 | Remote sources: `https://`, `rtmp://`, `rtsp://` | ⏳ next |

## Platforms

Linux (x86_64, aarch64) and macOS (x86_64, aarch64). Windows is not on the roadmap; PRs welcome.

## Install

> **Runtime requirement:** `ffmpeg` and `ffprobe` must be on `PATH`
> (Phase 2+; not needed for Phase 1's connect-only mode).
> Debian/Ubuntu: `sudo apt-get install -y ffmpeg`. macOS: `brew install ffmpeg`.

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

## Usage

```bash
# Local file
stream-to-agora ./demo.mp4 --app-id $AGORA_APP_ID --channel demo --rtc-user-id 42 --token "$TOKEN"

# String account (matches atem's "s/" convention)
stream-to-agora ./demo.mp4 --app-id ... --channel demo --rtc-user-id s/alice --token "$TOKEN"

# Loop forever for steady-state load testing
stream-to-agora ./loop.mp4 --app-id ... --channel demo --rtc-user-id 42 --token "$TOKEN" --loop

# Remote source (Phase 3)
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
┌──────────────┐  encoded NALUs+ADTS   ┌──────────────┐  encoded sender  ┌────────┐
│   ffmpeg     │ ────────────────────► │ stream-to-   │ ───────────────► │ Agora  │
│ (demux only) │                       │ agora (Rust) │                  │  RTC   │
│  -c copy     │                       │              │                  │        │
└──────────────┘                       │              │                  └────────┘
                                       │   matches    │
┌──────────────┐    raw YUV+PCM        │  CodecMode   │   raw sender     ┌────────┐
│   ffmpeg     │ ────────────────────► │  on startup  │ ───────────────► │ Agora  │
│ (decode)     │                       │              │                  │  RTC   │
│ -pix_fmt …   │                       └──────────────┘                  └────────┘
└──────────────┘
```

Mode is chosen at startup by `ffprobe`'ing the input: if both streams use
codecs Agora's encoded senders accept, ffmpeg is launched with `-c copy`
(zero-CPU demux); otherwise ffmpeg decodes and we push raw.

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
