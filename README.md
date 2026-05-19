# stream-to-agora

Stream a local file or `http(s)://` / `rtmp://` / `rtsp://` URL to an Agora RTC channel as a regular publisher. ffmpeg decodes/demuxes the source; codecs the SDK's encoded senders accept (H.264, H.265, VP8, VP9, AV1 video; AAC/HE-AAC/HE-AACv2, Opus, G.711 audio) pass through as-is, anything else gets decoded to raw YUV/PCM and pushed via the SDK's external-source APIs.

## Features

- **Local files** — any container/codec ffmpeg can read (`./demo.mp4`, `./loop.mkv`, …).
- **Remote sources** — `http://`, `https://`, `rtmp://`, `rtsp://`.
- **Encoded passthrough** — H.264/H.265/VP8/VP9/AV1 video and AAC/HE-AAC/HE-AACv2/Opus/G.711 audio are demuxed with `-c copy` (zero CPU on our side); the SDK gets the bitstream as-is. Mode is per-input all-or-nothing: both streams must be passthrough-eligible or the whole input falls back to Raw.
- **Raw fallback** — anything else (MP3, MPEG-2, AC-3, …) is decoded by ffmpeg to yuv420p + s16le PCM and pushed via the raw senders.
- **Selective publish** — `--audio-only` / `--video-only` for source previews, audio-only push-to-talk, etc.
- **Hybrid reconnect** — `http(s)` uses ffmpeg's built-in `-reconnect` flags; RTMP/RTSP respawn the ffmpeg subprocess, bounded by `--reconnect-attempts`.
- **Token renewal** — `--token-renew-cmd <shell-cmd>` runs your token-minter on `TokenWillExpire` and rotates the token without dropping the channel.
- **`--loop` forever** — steady-state load testing from a single short file.
- **ffmpeg passthrough flags** — `--http-header K:V` (repeatable), `--user-agent`, `--rtsp-transport tcp|udp|http`.

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

# Remote HTTPS source
stream-to-agora https://example.com/stream.mp4 \
  --app-id $AGORA_APP_ID --channel demo --rtc-user-id 42 --token "$TOKEN"

# RTMP ingest with reconnect (default --reconnect-attempts 5)
stream-to-agora rtmp://origin.example.com/live/demo \
  --app-id ... --channel demo --rtc-user-id 42 --token "$TOKEN"

# RTSP camera through a UDP-blocking NAT
stream-to-agora rtsp://camera.local/stream1 --rtsp-transport tcp \
  --app-id ... --channel demo --rtc-user-id 42 --token "$TOKEN"

# Audio-only publish from a video file
stream-to-agora ./talk.mp4 --audio-only \
  --app-id ... --channel demo --rtc-user-id 42 --token "$TOKEN"

# Auth'd HLS source with a Bearer token and custom UA
stream-to-agora https://secured.example/master.m3u8 \
  --http-header 'Authorization: Bearer abc123' \
  --user-agent 'stream-to-agora/0.3' \
  --app-id ... --channel demo --rtc-user-id 42 --token "$TOKEN"

# Long stream with auto-renew (90s tokens for fast iteration during dev)
TOKEN=$(atem token rtc create --channel demo --rtc-user-id 42 --expire 90 | awk '/^RTC Token/{getline; print; exit}')
stream-to-agora ./show.mp4 --loop --duration 600 \
  --token-renew-cmd 'atem token rtc create --channel {channel} --rtc-user-id {rtc_user_id} --expire 90 | awk "/^RTC Token created/{getline; print; exit}"' \
  --app-id ... --channel demo --rtc-user-id 42 --token "$TOKEN"
```

## Configuration consistency with atem

Same env-var name and flag conventions so a shell that has atem set up also has stream-to-agora set up:

| Setting | Source | Used by |
|---|---|---|
| App ID | `--app-id` flag or `AGORA_APP_ID` env | atem, stream-to-agora |
| Channel | `--channel` flag | atem, stream-to-agora |
| RTC user | `--rtc-user-id` (with `s/` prefix to force string account) | atem, stream-to-agora |

stream-to-agora does NOT read atem's encrypted credentials store or active project — it's intentionally standalone so it can be dropped on a fresh machine without atem installed.

### Token renewal

`--token-renew-cmd <shell command>` runs the given command via `sh -c …`
whenever the SDK signals that the active token is ~30 s from expiry.
The entire trimmed stdout of the command becomes the new token; the
command's stderr is logged on failure.

Placeholders supported inside the command string:
- `{channel}` — the channel name passed via `--channel`
- `{rtc_user_id}` — the user id passed via `--rtc-user-id`

The command should print exactly one token to stdout. For `atem`:

```bash
--token-renew-cmd 'atem token rtc create --channel {channel} --rtc-user-id {rtc_user_id} | awk "/^RTC Token created/{getline; print; exit}"'
```

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

### Encoded passthrough caveats

- **Mid-join keyframe latency.** Passthrough forwards the source bitstream
  verbatim — there is no encoder on our side, so a subscriber's
  intra-frame request (`onIntraRequestReceived`, sent by WebRTC clients on
  join / packet loss) cannot be honoured by re-encoding a keyframe. A
  subscriber who joins mid-stream sees black until the source's next
  keyframe arrives. For keyframe-dense sources this is sub-second; for
  sparse-GOP sources the gap is bounded by the source keyframe interval
  (a 4 s GOP → up to ~4 s of initial black). Raw mode is unaffected (the
  SDK's own encoder answers PLIs). To minimise mid-join black with
  passthrough, feed a source with frequent keyframes.
- **Subscriber decode support is codec-dependent.** The SDK transports
  every codec above, but the *subscriber* still has to decode it. H.264
  and VP8 decode in every WebRTC client. VP9 and AV1 decode in
  Chrome/Edge/Firefox but not Safari. H.265/HEVC only decodes in Safari
  and hardware-accelerated Chrome. AAC/HE-AAC/HE-AACv2/Opus/G.711 audio
  decode everywhere. The native Agora SDK subscriber decodes all of them.

The Agora NG SDK ships a flat C API (`agora_service_create`, `agora_rtc_conn_connect`, `agora_video_frame_sender_send`, …), which Rust links directly via `extern "C"` for everything except the encoded senders — see §Known Issues. The encoded path routes through a small C++ shim in `cpp/agora_shim.cpp`.

- `src/main.rs` — CLI, ffmpeg subprocess management, frame pacing
- `src/agora/` — safe Rust wrappers (`session.rs`, `video.rs`, `audio.rs`, …)
- `src/agora/shim.rs` — FFI declarations for the C++ encoded-sender shim
- `cpp/agora_shim.{h,cpp}` — C++ shim calling `IVideoEncodedImageSender::sendEncodedVideoImage` / `IAudioEncodedFrameSender::sendEncodedAudioFrame` directly
- `CMakeLists.txt` — downloads + stages the Agora SDK at build time, emits include/lib paths
- `build.rs` — runs CMake, compiles the C++ shim via `cc`, links `libagora_rtc_sdk` + the shim, sets rpath

## Known Issues

### SDK flat-C encoded senders are broken (worked around via C++ shim)

`agora_video_encoded_image_sender_send` and `agora_audio_encoded_frame_sender_send` in the SDK's flat C API accept exactly one frame and then return `false` for every subsequent call. The bug is in the C wrappers — the C++ methods (`IVideoEncodedImageSender::sendEncodedVideoImage`, `IAudioEncodedFrameSender::sendEncodedAudioFrame`) work correctly.

Verified by running the SDK's own `sample_send_h264_pcm` with return-value logging (the upstream sample discards the return value, hiding the bug): 90/90 frames accepted via C++. A minimal Rust harness calling the flat C functions with the exact same setup gets 1/N accepted.

**Workaround in this repo:** `cpp/agora_shim.cpp` exposes a narrow `extern "C"` ABI that constructs the encoded sender + custom track via the C++ API and forwards `send` calls through the C++ vtable. Rust calls into this shim instead of the broken flat-C functions. The raw senders (`agora_video_frame_sender_send`, `agora_audio_pcm_data_sender_send`) work fine through the flat C API and are unchanged.

The shim takes the existing flat-C service/connection/factory handles (which Rust holds via `bindgen`) and recovers the underlying C++ object pointers by dereferencing them — the flat-C wrappers store the C++ object pointer in the first 8 bytes of the handle struct (verified via disassembly of the SDK's own C wrappers).

If/when Agora fixes the C ABI, the shim can be deleted and the encoded publishers in `src/agora/{video,audio}.rs` can switch back to the flat-C `agora_*_sender_send` calls.

### Resolved: encoded video rendered black at subscribers

Earlier the encoded path connected and the SDK accepted every frame, yet
WebRTC subscribers showed black video and flooded the sender with
intra-frame (keyframe) requests; audio was unaffected. Root cause was the
H.264 access-unit splitter (`parse::h264::next_au`): it cut a new frame at
every *second* VCL slice, so at each GOP boundary the IDR was emitted
*without* its SPS/PPS (those got glued onto the preceding delta frame).
A decoder can't initialize from an IDR with no parameter sets, so it never
produced a frame and kept asking for a keyframe.

Fixed by making the splitter group SPS+PPS+IDR into one keyframe access
unit — byte-for-byte matching the SDK sample's
`HelperH264FileParser::getH264Frame` (boundary at the next non-VCL NAL or
a slice with `first_mb_in_slice == 0`). Verified end-to-end against the
same channel as the SDK sample: subscriber decodes, no intra-request
flood. The shim no longer strips SEI or overrides `captureTimeMs` — both
were earlier mis-diagnoses that deviated from the working sample.

### Resolved: VP8/VP9/AV1 passthrough sent the SDK mislabeled bitstream

When VP8/VP9/AV1 encoded passthrough was first added, `video_codec_type`
(`src/agora/video.rs`) only mapped `hevc`; every other non-H.264 codec
fell through to `0`, so the shim told the SDK the bytes were H.264. The
SDK accepted every frame (`rc=ok`) but emitted an undecodable stream —
subscribers saw no video at all (not even black). It went unnoticed
because no test exercised `video_codec_type` and the e2e harness only
checks publisher liveness, never subscriber decode.

Fixed by mapping every passthrough codec to its real `VIDEO_CODEC_TYPE`
enum (VP8=1, H265=3, AV1=12, VP9=13). Guarded by
`video_codec_type_maps_every_encoded_codec` (and the matching
`audio_codec_maps_codec_and_profile`) so a regression fails `cargo test`.
The env-gated `STA_TRACE=1` shim diagnostic (`shim.vid[N] codec=… WxH
rc=…`) was retained — it is what localised this bug and pins the
codec/dimensions actually handed to the SDK.

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

### End-to-end tests

`scripts/test-e2e.sh` runs a matrix of live publish scenarios against
real Agora (local file + HTTPS MP4 + HLS + selective publish + CLI
validation). It requires `atem` on PATH with a project selected, plus
network reachability to media.w3.org, stream.mux.com, and Apple's CDN.

```bash
cargo build --release
scripts/test-e2e.sh        # ~3 min wall-clock, 16 tests
```

Override the binary path or per-test duration via env:

```bash
STREAM_TO_AGORA=/path/to/stream-to-agora DURATION=4 scripts/test-e2e.sh
```

## License

MIT.
