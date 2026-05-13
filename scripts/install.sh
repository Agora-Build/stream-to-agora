#!/usr/bin/env bash
#
# Install stream-to-agora — Agora's CLI for pushing media files to RTC channels.
#
# Quick install (works in regions where GitHub is slow):
#   curl -fsSL https://dl.agora.build/stream-to-agora/install.sh | bash
#
# Options (via env vars):
#   STA_VERSION=0.1.0     Pin a specific version (default: latest)
#   STA_BASE_URL=...      Override download base URL
#   STA_INSTALL_DIR=...   Override install directory (binary)
#   STA_LIB_DIR=...       Override SDK library directory (default: <install_dir>/../lib/stream-to-agora)
#
set -euo pipefail

BASE_URL="${STA_BASE_URL:-https://dl.agora.build/stream-to-agora/releases}"

# ── Helpers ──────────────────────────────────────────────────────────

die()  { echo "error: $*" >&2; exit 1; }
info() { echo "  $*"; }

# ── Detect platform ──────────────────────────────────────────────────

detect_platform() {
  local os arch
  os="$(uname -s | tr '[:upper:]' '[:lower:]')"
  arch="$(uname -m)"

  case "$os" in
    linux)  os="linux" ;;
    darwin) os="darwin" ;;
    *)      die "Unsupported OS: $os (supported: linux, darwin)" ;;
  esac

  case "$arch" in
    x86_64|amd64)   arch="x86_64" ;;
    aarch64|arm64)  arch="aarch64" ;;
    *)              die "Unsupported architecture: $arch (supported: x86_64, aarch64)" ;;
  esac

  echo "${os}-${arch}"
}

# ── Resolve version ─────────────────────────────────────────────────

resolve_version() {
  if [ -n "${STA_VERSION:-}" ]; then
    echo "$STA_VERSION"
    return
  fi

  local url="${BASE_URL}/latest"
  local version
  version="$(curl -fsSL "$url" 2>/dev/null)" \
    || die "Failed to fetch latest version from $url"
  version="$(echo "$version" | tr -d '[:space:]')"
  [ -n "$version" ] || die "Empty version returned from $url"
  echo "$version"
}

# ── Pick install directories ────────────────────────────────────────
# Binary goes in PATH; SDK shared libs go beside it (`../lib/stream-to-agora`)
# so the binary's rpath ($ORIGIN/../lib or @loader_path/../lib) finds them.

pick_install_dir() {
  if [ -n "${STA_INSTALL_DIR:-}" ]; then
    mkdir -p "$STA_INSTALL_DIR"
    echo "$STA_INSTALL_DIR"
    return
  fi

  if [ -w "/usr/local/bin" ]; then
    echo "/usr/local/bin"
  else
    local dir="${HOME}/.local/bin"
    mkdir -p "$dir"
    echo "$dir"
  fi
}

pick_lib_dir() {
  local install_dir="$1"
  if [ -n "${STA_LIB_DIR:-}" ]; then
    mkdir -p "$STA_LIB_DIR"
    echo "$STA_LIB_DIR"
    return
  fi
  # Sibling lib/ — peer to bin/. e.g. /usr/local/bin → /usr/local/lib/stream-to-agora
  local dir
  dir="$(dirname "$install_dir")/lib/stream-to-agora"
  mkdir -p "$dir"
  echo "$dir"
}

# ── Main ─────────────────────────────────────────────────────────────

main() {
  echo "Installing stream-to-agora..."

  local platform version install_dir lib_dir
  platform="$(detect_platform)"

  # The Agora RTSA SDK currently ships only for x86_64 Linux, so that's
  # the only prebuilt we publish. Other platforms will be added when the
  # corresponding SDK builds become available.
  if [ "$platform" != "linux-x86_64" ]; then
    die "No prebuilt available for ${platform} yet — stream-to-agora currently supports linux-x86_64 only. Build from source: https://github.com/Agora-Build/stream-to-agora"
  fi

  version="$(resolve_version)"
  install_dir="$(pick_install_dir)"
  lib_dir="$(pick_lib_dir "$install_dir")"

  local archive="stream-to-agora-v${version}-${platform}.tar.gz"
  local url="${BASE_URL}/v${version}/${archive}"

  info "Version:  ${version}"
  info "Platform: ${platform}"
  info "From:     ${url}"
  info "Bin to:   ${install_dir}/stream-to-agora"
  info "Lib to:   ${lib_dir}/"

  tmpdir="$(mktemp -d)"
  trap 'rm -rf "$tmpdir"' EXIT

  curl -fSL --progress-bar "$url" -o "${tmpdir}/${archive}" \
    || die "Download failed: ${url}"

  tar -xzf "${tmpdir}/${archive}" -C "$tmpdir" \
    || die "Failed to extract ${archive}"

  # Tarball layout: stream-to-agora/{bin/stream-to-agora,lib/lib*.so}
  local payload="${tmpdir}/stream-to-agora"
  [ -f "${payload}/bin/stream-to-agora" ] \
    || die "Tarball is missing bin/stream-to-agora; corrupted release?"

  chmod +x "${payload}/bin/stream-to-agora"
  mv "${payload}/bin/stream-to-agora" "${install_dir}/stream-to-agora" \
    || die "Failed to install to ${install_dir}/stream-to-agora (try sudo or set STA_INSTALL_DIR)"

  # Copy SDK shared libs. We mv the whole tree so symlinks etc. survive.
  if [ -d "${payload}/lib" ]; then
    cp -a "${payload}/lib/." "${lib_dir}/" \
      || die "Failed to install SDK libraries to ${lib_dir}"
  fi

  # Verify
  echo ""
  if command -v stream-to-agora >/dev/null 2>&1; then
    local installed_version
    installed_version="$(stream-to-agora --version 2>/dev/null | head -1)" || installed_version="(version check failed — likely missing SDK libs)"
    echo "Installed: ${installed_version}"
  else
    echo "Installed to ${install_dir}/stream-to-agora"
    case ":${PATH}:" in
      *":${install_dir}:"*) ;;
      *)
        echo ""
        echo "Add to your PATH:"
        echo "  export PATH=\"${install_dir}:\$PATH\""
        ;;
    esac
  fi
}

main "$@"
