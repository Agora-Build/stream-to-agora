# @agora-build/stream-to-agora

Stream a local file or `http(s)://` / `rtmp://` / `rtsp://` URL to an [Agora](https://www.agora.io) RTC channel as a regular publisher. ffmpeg decodes/demuxes the source; codecs the SDK's encoded senders accept (H.264, H.265, VP8, VP9, AV1 video; AAC/HE-AAC/HE-AACv2, Opus, G.711 audio) pass through as-is, anything else is decoded to raw YUV/PCM and pushed via the raw senders — useful for load testing, demos, simulated participants, and pumping pre-recorded media into a live channel.

## Features

- **Local files** — any container/codec ffmpeg can read (`./demo.mp4`, `./loop.mkv`, …).
- **Remote sources** — `http://`, `https://`, `rtmp://`, `rtsp://`.
- **Encoded passthrough** — H.264/H.265/VP8/VP9/AV1 video and AAC/HE-AAC/HE-AACv2/Opus/G.711 audio are demuxed with `-c copy` (zero CPU on our side); the SDK gets the bitstream as-is. Mode is per-input all-or-nothing: both streams must be passthrough-eligible or the whole input falls back to Raw.
- **Raw fallback** — anything else (MP3, MPEG-2, AC-3, …) is decoded by ffmpeg to yuv420p + s16le PCM and pushed via the raw senders.
- **Selective publish** — `--audio-only` / `--video-only`.
- **Hybrid reconnect** — `http(s)` uses ffmpeg's built-in `-reconnect` flags; RTMP/RTSP respawn the ffmpeg subprocess, bounded by `--reconnect-attempts`.
- **Token renewal** — `--token-renew-cmd <shell-cmd>` runs your token-minter on `TokenWillExpire` and rotates the token without dropping the channel.
- **`--loop` forever** — steady-state load testing from a single short file.
- **ffmpeg passthrough flags** — `--http-header K:V` (repeatable), `--user-agent`, `--rtsp-transport tcp|udp|http`.

### Encoded passthrough caveats

- **VP9 and AV1 currently don't render** at subscribers on RTSA 4.4.32: the SDK accepts the frames but emits no RTP for those codecs (verified with Agora's own sample sender + native receiver — VP8 control passes). The publisher side is correct and will start rendering automatically once Agora wires the missing packetizers; until then prefer H.264 / H.265 / VP8 for video.
- Subscriber decode support is codec-dependent: H.264/VP8 every WebRTC client; H.265 Safari + hardware-accelerated Chrome; VP9/AV1 Chrome/Edge/Firefox (when the SDK ships them). Agora's native SDK subscriber decodes all.
- Passthrough cannot synthesise keyframes to answer `onIntraRequestReceived`; mid-join black at a browser subscriber is bounded by the source's keyframe interval (sub-second for keyframe-dense inputs).

## Install

```bash
npm install -g @agora-build/stream-to-agora
```

Or via shell script:

```bash
curl -fsSL https://dl.agora.build/stream-to-agora/install.sh | bash
```

Both download a prebuilt bundle for your platform (currently `linux-x86_64` and `linux-aarch64`; macOS not yet released — `cargo build --release` from source still works on Apple Silicon and Intel) — the binary plus the Agora SDK shared libraries it depends on. The binary's rpath finds the libs at runtime, so there's no `LD_LIBRARY_PATH` / `DYLD_LIBRARY_PATH` setup.

## Usage

```bash
stream-to-agora <INPUT> --app-id <ID> --channel <NAME> --rtc-user-id <UID> --token <TOKEN> [OPTIONS]
```

```bash
# Local file
stream-to-agora ./demo.mp4 \
  --app-id      $AGORA_APP_ID \
  --channel     demo \
  --rtc-user-id 42 \
  --token       "$RTC_TOKEN"

# String account (same "s/" convention as `atem serv rtc`)
stream-to-agora ./demo.mp4 --app-id ... --channel demo --rtc-user-id s/alice --token "$RTC_TOKEN"

# Loop forever — steady-state load testing
stream-to-agora ./loop.mp4 --app-id ... --channel demo --rtc-user-id 42 --token "$RTC_TOKEN" --loop

# Audio only / video only
stream-to-agora ./demo.mp4 --app-id ... --channel demo --rtc-user-id 42 --token "$RTC_TOKEN" --audio-only

# Remote source (later release — ffmpeg already handles these, so the arg shape is unchanged)
stream-to-agora rtmp://live.example.com/app/key --app-id ... --channel demo --rtc-user-id 42 --token "$RTC_TOKEN"
```

### Options

| Flag | Description |
|---|---|
| `--app-id <ID>` | Agora App ID. Falls back to `AGORA_APP_ID` env var. |
| `--channel <NAME>` | RTC channel to join. |
| `--rtc-user-id <UID>` | RTC user. All-digit → int uid; non-digit → string account; `s/` prefix forces string mode. |
| `--token <TOKEN>` | Pre-minted RTC token (required). |
| `--mode <auto\|raw\|encoded>` | Sender path (default `auto`). See **Modes** below. |
| `--loop` | Restart the input on EOF. |
| `--audio-only` / `--video-only` | Push just one track. |
| `--ffmpeg-path <PATH>` | ffmpeg binary (default: `ffmpeg` on `PATH`). |

### Modes

| `--mode` | Behaviour |
|---|---|
| `auto` (default) | Encoded passthrough when every stream's codec is passthrough-eligible, else Raw. |
| `raw` | Force ffmpeg-decode → SDK-re-encode for every input. More CPU + one re-encode generation, but the SDK's internal encoder owns the bitstream — it answers subscriber keyframe requests (PLI) itself, so mid-join subscribers aren't stuck on black, and it works for any codec ffmpeg decodes, including VP9/AV1. |
| `encoded` | Force zero-CPU passthrough; errors at startup (naming the codec) if a stream can't pass through, instead of silently falling back. |

### Tokens

stream-to-agora **does not mint tokens** — token minting is a security-sensitive concern that belongs in your token service or [atem](https://github.com/Agora-Build/Atem). Supply a pre-minted token:

```bash
TOKEN=$(atem token rtc create --channel demo --rtc-user-id 42)
stream-to-agora ./demo.mp4 --app-id $AGORA_APP_ID --channel demo --rtc-user-id 42 --token "$TOKEN"
```

Two publishers on the same channel must use different uids (Agora kicks a duplicate uid). Same channel + different uids = fine.

## Requirements

- **ffmpeg** on `PATH` (or pass `--ffmpeg-path`). Used to decode the input into raw frames.
- The project must have an App Certificate and the token must be valid for the channel + uid + publisher role.

## Supported Platforms

| Platform | Source build | Released binary |
|----------|--------------|-----------------|
| Linux x86_64 | ✅ | ✅ |
| Linux aarch64 | ✅ | ✅ |
| macOS arm64 / x86_64 | ✅ | ❌ (macOS RTSA tarball URL not yet verified) |
| Windows | — | — (not on the roadmap) |

## Build from Source

```bash
git clone https://github.com/Agora-Build/stream-to-agora.git
cd stream-to-agora
cargo build --release         # CMake fetches the Agora SDK on first build
# Binary at target/release/stream-to-agora
```

To skip the SDK auto-download and point at a pre-staged SDK:

```bash
AGORA_RTC_SDK_PATH=/path/to/agora_rtc_sdk cargo build --release
```

## Related Projects

- [atem](https://github.com/Agora-Build/Atem) — Agora platform terminal: projects, tokens, ConvoAI test pages, webhook receiver, AI agent integration
- [Astation](https://github.com/Agora-Build/Astation) — macOS menubar hub coordinating Chisel, atem, and AI agents
- [Vox](https://github.com/Agora-Build/Vox) — AI latency evaluation platform

## License

MIT
