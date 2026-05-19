#!/usr/bin/env bash
# Regenerate every fixture in tests/fixtures/ from its ffmpeg recipe.
#
# Reproducibility: each fixture is built with a deterministic ffmpeg
# command. Re-running this script after pulling the repo should produce
# byte-similar fixtures (libx264 isn't fully deterministic across
# versions, so byte-exact isn't guaranteed — but stream structure is).
#
# Usage: scripts/regen-fixtures.sh                  # rebuild all
#        scripts/regen-fixtures.sh motion-pattern-5s  # rebuild just one (basename)
#
# Required: ffmpeg with libx264 + libx265 + aac + libopus + lavfi.
# Network: needed for `bbb-30s` and `sintel-15s` (Blender open movies).

set -euo pipefail

cd "$(dirname "$0")/.."
mkdir -p tests/fixtures
cd tests/fixtures

X264_BASELINE='-c:v libx264 -preset medium -profile:v baseline -level 3.0 -pix_fmt yuv420p'

regen_motion_pattern_5s() {
    echo "→ motion-pattern-5s.mp4"
    ffmpeg -hide_banner -loglevel error -y \
      -f lavfi -i "mptestsrc=duration=5:rate=15" \
      -f lavfi -i "aevalsrc=0.3*sin(440*2*PI*t)+0.3*sin(554*2*PI*t)+0.3*sin(659*2*PI*t):d=5" \
      $X264_BASELINE \
      -x264-params "slices=1:keyint=15" \
      -c:a aac -ar 48000 -ac 1 -b:a 96k -shortest \
      motion-pattern-5s.mp4
}

regen_smptebars_5s() {
    echo "→ smptebars-30fps-stereo-5s.mp4"
    ffmpeg -hide_banner -loglevel error -y \
      -f lavfi -i "smptebars=duration=5:rate=30:size=480x270" \
      -f lavfi -i "sine=frequency=1000:duration=5" \
      $X264_BASELINE \
      -x264-params "slices=1:keyint=30" \
      -c:a aac -ar 48000 -ac 2 -b:a 128k -shortest \
      smptebars-30fps-stereo-5s.mp4
}

regen_bbb_30s() {
    echo "→ bbb-30s.mp4 (downloading from blender.org)"
    ffmpeg -hide_banner -loglevel error -y \
      -ss 60 -t 30 -i https://download.blender.org/peach/bigbuckbunny_movies/BigBuckBunny_320x180.mp4 \
      $X264_BASELINE \
      -x264-params "slices=1:keyint=15:min-keyint=15" \
      -c:a aac -ar 48000 -ac 1 -b:a 96k \
      bbb-30s.mp4
}

regen_sintel_15s() {
    echo "→ sintel-15s.mp4 (downloading from blender.org)"
    ffmpeg -hide_banner -loglevel error -y \
      -ss 5 -t 15 -i https://download.blender.org/durian/trailer/sintel_trailer-480p.mp4 \
      $X264_BASELINE -vf "scale=480:-2" \
      -x264-params "slices=1:keyint=24" \
      -c:a aac -ar 48000 -ac 2 -b:a 128k \
      sintel-15s.mp4
}

regen_hevc_opus_5s() {
    echo "→ hevc-opus-5s.mp4 (H.265 + Opus encoded-passthrough fixture)"
    ffmpeg -hide_banner -loglevel error -y \
      -f lavfi -i "testsrc=duration=5:size=320x240:rate=15" \
      -f lavfi -i "sine=frequency=440:duration=5" \
      -c:v libx265 -preset medium -pix_fmt yuv420p -tag:v hvc1 \
      -x265-params "log-level=error:keyint=15:min-keyint=15" \
      -c:a libopus -ar 48000 -ac 1 -b:a 64k -shortest \
      hevc-opus-5s.mp4
}

regen_vp8_opus_5s() {
    echo "→ vp8-opus-5s.webm (VP8 + Opus encoded-passthrough fixture)"
    ffmpeg -hide_banner -loglevel error -y \
      -f lavfi -i "testsrc=duration=5:size=320x240:rate=15" \
      -f lavfi -i "sine=frequency=440:duration=5" \
      -c:v libvpx -b:v 300k -g 15 -keyint_min 15 -deadline realtime \
      -c:a libopus -ar 48000 -ac 1 -b:a 64k -shortest \
      vp8-opus-5s.webm
}

regen_vp9_opus_5s() {
    echo "→ vp9-opus-5s.webm (VP9 + Opus encoded-passthrough fixture)"
    ffmpeg -hide_banner -loglevel error -y \
      -f lavfi -i "testsrc=duration=5:size=320x240:rate=15" \
      -f lavfi -i "sine=frequency=440:duration=5" \
      -c:v libvpx-vp9 -b:v 300k -g 15 -keyint_min 15 -deadline realtime \
      -c:a libopus -ar 48000 -ac 1 -b:a 64k -shortest \
      vp9-opus-5s.webm
}

regen_av1_aac_5s() {
    echo "→ av1-aac-5s.mp4 (AV1 + AAC encoded-passthrough fixture)"
    ffmpeg -hide_banner -loglevel error -y \
      -f lavfi -i "testsrc=duration=5:size=320x240:rate=15" \
      -f lavfi -i "sine=frequency=440:duration=5" \
      -c:v libsvtav1 -preset 10 -g 15 \
      -c:a aac -ar 48000 -ac 1 -b:a 96k -shortest \
      av1-aac-5s.mp4
}

regen_h264_g711u_5s() {
    echo "→ h264-g711u-5s.mkv (H.264 + G.711 µ-law encoded-passthrough fixture)"
    ffmpeg -hide_banner -loglevel error -y \
      -f lavfi -i "testsrc=duration=5:size=320x240:rate=15" \
      -f lavfi -i "sine=frequency=440:duration=5" \
      $X264_BASELINE -x264-params "slices=1:keyint=15" \
      -c:a pcm_mulaw -ar 8000 -ac 1 -shortest \
      h264-g711u-5s.mkv
}

# NOTE: an HE-AAC fixture requires an ffmpeg built with libfdk_aac
# (`-c:a libfdk_aac -profile:a aac_he`); the stock encoder has no HE
# profile. The HE-AAC code path is covered by `src/agora/audio.rs`
# unit tests + `decide()` coverage; live-verify with a real HE-AAC
# source when an fdk-enabled ffmpeg is available.

ALL="motion-pattern-5s smptebars-30fps-stereo-5s bbb-30s sintel-15s hevc-opus-5s vp8-opus-5s vp9-opus-5s av1-aac-5s h264-g711u-5s"

if [ $# -eq 0 ]; then
    for f in $ALL; do
        # Convert basename → function name (replace - with _)
        fn="regen_$(echo "$f" | tr '-' '_')"
        $fn
    done
else
    for arg in "$@"; do
        fn="regen_$(echo "${arg%.mp4}" | tr '-' '_')"
        $fn
    done
fi

echo ""
echo "=== Result ==="
for f in *.mp4; do
    printf "  %-36s " "$f"
    ffprobe -v error -show_entries 'stream=codec_name,width,height,sample_rate,channels:format=duration,size' -of csv=p=0 "$f" | tr '\n' ' '
    echo ""
done
