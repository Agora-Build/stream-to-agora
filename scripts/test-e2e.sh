#!/usr/bin/env bash
# Phase 3 end-to-end test harness.
#
# Runs stream-to-agora against a matrix of inputs (local file, HTTPS MP4,
# HLS, --audio-only/--video-only, --loop, --http-header) and reports
# pass/fail. Each test spends ~6s of wall-clock time on the wire so the
# full run takes ~3 min over a typical home connection.
#
# Requires:
#   - `atem` on PATH with a project selected (`atem project use …`)
#     — used to fetch the App ID and mint short-lived RTC tokens.
#   - `target/release/stream-to-agora` already built (`cargo build --release`).
#   - Network reachability to media.w3.org / stream.mux.com / Apple CDN.
#
# Output:
#   PASS <id>  — `stream-to-agora` exited 0 and printed `disconnected.`
#   FAIL <id>  — exited non-zero, timed out, or did not reach `disconnected.`
#
# Exit code: 0 if all PASS, 1 otherwise.

set -u

BIN="${STREAM_TO_AGORA:-./target/release/stream-to-agora}"
DURATION="${DURATION:-6}"
TIMEOUT="${TIMEOUT:-30}"

if [ ! -x "$BIN" ]; then
  echo "ERROR: $BIN not found or not executable." >&2
  echo "Build first:  cargo build --release" >&2
  exit 2
fi

if ! command -v atem >/dev/null 2>&1; then
  echo "ERROR: atem not on PATH — needed for App ID + token minting." >&2
  exit 2
fi

APP_ID=$(atem project show 2>&1 | awk '/^App ID:/ {print $3}')
if [ -z "$APP_ID" ]; then
  echo "ERROR: could not resolve App ID from \`atem project show\`." >&2
  echo "Run \`atem project use <project>\` to select one." >&2
  exit 2
fi
echo "App ID:  ${APP_ID:0:4}…${APP_ID: -4}"
echo "Binary:  $BIN"
echo ""

PASS=0
FAIL=0
FAILED_TESTS=()

mktoken() {
  atem token rtc create --channel "$1" --rtc-user-id "$2" --expire 600 2>&1 | sed -n '2p'
}

# run <test-id> <input> [extra-flags...]
# Expects clean `disconnected.` exit within DURATION+padding.
run() {
  local id="$1"; shift
  local input="$1"; shift
  local ch="p3e2e-${id}"
  local tok
  tok=$(mktoken "$ch" 99999)
  local out rc
  out=$(timeout "$TIMEOUT" "$BIN" "$input" \
    --app-id "$APP_ID" --channel "$ch" --rtc-user-id 99999 --token "$tok" \
    --duration "$DURATION" "$@" 2>&1)
  rc=$?
  if echo "$out" | grep -q "^disconnected\.$" && [ $rc -eq 0 ]; then
    echo "PASS  $id"
    PASS=$((PASS+1))
  else
    echo "FAIL  $id  (rc=$rc)"
    echo "$out" | tail -3 | sed 's/^/      /'
    FAIL=$((FAIL+1))
    FAILED_TESTS+=("$id")
  fi
}

# fail_run <test-id> <input> [extra-flags...]
# Expects non-zero exit (CLI validation rejection).
fail_run() {
  local id="$1"; shift
  local input="$1"; shift
  local ch="p3e2e-${id}"
  local tok
  tok=$(mktoken "$ch" 99999 2>/dev/null)
  local out rc
  out=$(timeout 10 "$BIN" "$input" \
    --app-id "$APP_ID" --channel "$ch" --rtc-user-id 99999 --token "${tok:-dummy}" \
    --duration 3 "$@" 2>&1)
  rc=$?
  if [ $rc -ne 0 ]; then
    echo "PASS  $id  (rejected, rc=$rc)"
    PASS=$((PASS+1))
  else
    echo "FAIL  $id  (expected non-zero exit)"
    echo "$out" | tail -3 | sed 's/^/      /'
    FAIL=$((FAIL+1))
    FAILED_TESTS+=("$id")
  fi
}

echo "─── Local file ───"
run local-encoded       tests/fixtures/loop-3s.mp4
run local-hevc-opus     tests/fixtures/hevc-opus-5s.mp4
run local-audio-only    tests/fixtures/loop-3s.mp4 --audio-only
run local-video-only    tests/fixtures/loop-3s.mp4 --video-only
run local-loop          tests/fixtures/loop-3s.mp4 --loop

echo ""
echo "─── HTTPS MP4 ───"
run https-mp4-both      https://media.w3.org/2010/05/sintel/trailer.mp4
run https-mp4-audio     https://media.w3.org/2010/05/sintel/trailer.mp4 --audio-only
run https-mp4-video     https://media.w3.org/2010/05/sintel/trailer.mp4 --video-only
run https-mp4-h264only  https://test-videos.co.uk/vids/bigbuckbunny/mp4/h264/360/Big_Buck_Bunny_360_10s_1MB.mp4 --video-only
# 4K HEVC Main-profile, 25 fps, video-only — exercises the H.265
# encoded-passthrough path (parse::hevc, -f hevc) from a remote URL.
run https-hevc-4k       https://lf-tk-sg.ibytedtos.com/obj/tcs-client-sg/resources/hevc_4k25P_main_1.mp4 --video-only

echo ""
echo "─── HTTPS HLS ───"
run hls-mux-48k         "https://stream.mux.com/v69RSHhFelSm4701snP22dYz2jICy4E4FUyk02rW4gxRM.m3u8"
# Apple bipbop has 22.05kHz stereo AAC which the SDK encoded sender
# currently rejects; video-only avoids that. Drop --video-only when the
# audio-rate quirk is resolved.
run hls-bipbop-vo       "https://devstreaming-cdn.apple.com/videos/streaming/examples/bipbop_16x9/bipbop_16x9_variant.m3u8" --video-only

echo ""
echo "─── HTTPS with --http-header / --user-agent ───"
run https-with-header   https://media.w3.org/2010/05/sintel/trailer.mp4 \
                        --http-header "X-Test: 1" --user-agent "stream-to-agora-e2e/0.1"

echo ""
echo "─── CLI validation (expected fails) ───"
fail_run bad-scheme                 "ftp://example.com/foo.mp4"
fail_run bad-header-no-colon        tests/fixtures/loop-3s.mp4 --http-header "no-colon-here"
fail_run bad-header-crlf            tests/fixtures/loop-3s.mp4 --http-header $'Bad: x\r\nInjected: y'
fail_run mutually-exclusive         tests/fixtures/loop-3s.mp4 --audio-only --video-only
fail_run missing-input              /tmp/this-file-does-not-exist.mp4

echo ""
echo "═══════════════════════════"
echo "Total: PASS=$PASS  FAIL=$FAIL"
if [ $FAIL -gt 0 ]; then
  echo "Failed:"
  printf '  %s\n' "${FAILED_TESTS[@]}"
  exit 1
fi
