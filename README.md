# stream-to-agora

Stream a local file (and later https/rtmp/rtsp) to an Agora RTC channel as a regular publisher. Ffmpeg decodes the source; raw YUV (video) and PCM (audio) frames are pushed to Agora via the RTC SDK's external-source APIs.

## Status

**v0.1 — Phase 0: scaffold only.** The CLI parses, validates args, mints an RTC token, and prints what it would do. No SDK calls yet. The next milestones add the real plumbing.

| Phase | Milestone | Status |
|---|---|---|
| 0 | CLI surface, token mint, arg validation | ✅ this commit |
| 1 | Agora RTC SDK loads, joins channel, logs "ready", idles | ⏳ next |
| 2 | Stream a static H.264 + AAC file end-to-end | ⏳ |
| 3 | Arbitrary file via ffmpeg pipeline (any codec ffmpeg decodes) | ⏳ |
| 4 | Remote sources: `https://`, `rtmp://`, `rtsp://` | ⏳ |

## Platforms

Linux (x86_64, aarch64) and macOS (x86_64, aarch64). Windows is not on the roadmap; PRs welcome.

## Install

Until Phase 1 ships there's no useful binary. To build the scaffold from source:

```bash
git clone <this repo>
cd stream-to-agora
cargo build --release
./target/release/stream-to-agora --help
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
┌─────────────┐    raw YUV     ┌─────────────┐    join+pushVideo    ┌────────┐
│   ffmpeg    │ ─────────────→ │  stream-to- │ ──────────────────→ │ Agora  │
│  (decoder)  │    raw PCM     │   agora     │    join+pushAudio   │  RTC   │
└─────────────┘ ─────────────→ │  (Rust+FFI) │ ──────────────────→ └────────┘
                                └──────┬──────┘
                                       │ FFI (C ABI)
                                       ▼
                                ┌─────────────┐
                                │ agora_shim  │  C++ wrapper around
                                │   (C++)     │  the Agora RTC SDK
                                └─────────────┘
```

- `src/main.rs` — Rust CLI, ffmpeg subprocess management, frame pacing
- `native/src/agora_shim.cpp` — thin C++ wrapper over Agora's C++ SDK
- `native/include/agora_shim.h` — C ABI exported to Rust
- `build.rs` — locates/downloads the SDK at build time, compiles the shim

## Development

```bash
cargo build              # debug
cargo test               # unit tests (CLI parsing, input classification)
cargo clippy             # lint
cargo run -- ./demo.mp4 --app-id $APP --app-certificate $CERT --channel test --rtc-user-id 42
```

## License

MIT.
