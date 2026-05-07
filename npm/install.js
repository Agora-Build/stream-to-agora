#!/usr/bin/env node
"use strict";

// Postinstall — fetches the platform-specific tarball from the GitHub
// release for this version and unpacks it into ./bin (a `bin/` for
// the executable plus a sibling `lib/` for the Agora SDK shared
// libraries). The binary's rpath includes `$ORIGIN/../lib` /
// `@loader_path/../lib` so it finds the SDK at runtime without
// LD_LIBRARY_PATH gymnastics.

const https = require("https");
const http = require("http");
const fs = require("fs");
const path = require("path");
const { execSync } = require("child_process");
const os = require("os");

const pkg = require("./package.json");
const VERSION = `v${pkg.version}`;
const REPO = "Agora-Build/stream-to-agora";
const ROOT_DIR = __dirname;        // npm package root after install
const BIN_DIR  = path.join(ROOT_DIR, "bin");
const LIB_DIR  = path.join(ROOT_DIR, "lib");
const BIN_PATH = path.join(BIN_DIR, "stream-to-agora");

function getPlatformKey() {
  const platform = os.platform();
  const arch = os.arch();

  const map = {
    "linux-x64":   "linux-x86_64",
    "linux-arm64": "linux-aarch64",
    "darwin-x64":  "darwin-x86_64",
    "darwin-arm64":"darwin-aarch64",
  };

  const key = `${platform}-${arch}`;
  if (!map[key]) {
    console.error(`Unsupported platform: ${key}`);
    console.error("Supported: linux-x64, linux-arm64, darwin-x64, darwin-arm64");
    process.exit(1);
  }
  return map[key];
}

function getDownloadUrl() {
  const platformKey = getPlatformKey();
  return `https://github.com/${REPO}/releases/download/${VERSION}/stream-to-agora-${VERSION}-${platformKey}.tar.gz`;
}

function fetch(url, redirects = 0) {
  if (redirects > 5) {
    return Promise.reject(new Error("Too many redirects"));
  }
  return new Promise((resolve, reject) => {
    const client = url.startsWith("https") ? https : http;
    client
      .get(url, { headers: { "User-Agent": "stream-to-agora-npm-installer" } }, (res) => {
        if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
          return resolve(fetch(res.headers.location, redirects + 1));
        }
        if (res.statusCode !== 200) {
          return reject(new Error(`Download failed: HTTP ${res.statusCode} for ${url}`));
        }
        const chunks = [];
        res.on("data", (chunk) => chunks.push(chunk));
        res.on("end", () => resolve(Buffer.concat(chunks)));
        res.on("error", reject);
      })
      .on("error", reject);
  });
}

async function install() {
  const url = getDownloadUrl();
  console.log(`Downloading stream-to-agora ${VERSION} for ${getPlatformKey()}...`);
  console.log(`  ${url}`);

  const tarball = await fetch(url);

  const tmpFile = path.join(os.tmpdir(), `stream-to-agora-${Date.now()}.tar.gz`);
  fs.writeFileSync(tmpFile, tarball);

  // Tarball layout produced by .github/workflows/release.yml:
  //   stream-to-agora/bin/stream-to-agora
  //   stream-to-agora/lib/libagora_rtc_sdk.so (and friends)
  // Strip the top-level dir so files land under ROOT_DIR/{bin,lib}.
  fs.mkdirSync(BIN_DIR, { recursive: true });
  fs.mkdirSync(LIB_DIR, { recursive: true });

  try {
    execSync(`tar -xzf "${tmpFile}" --strip-components=1 -C "${ROOT_DIR}"`, { stdio: "pipe" });
  } finally {
    fs.unlinkSync(tmpFile);
  }

  fs.chmodSync(BIN_PATH, 0o755);
  console.log(`Installed stream-to-agora ${VERSION} to ${BIN_PATH}`);
}

install().catch((err) => {
  console.error(`Failed to install stream-to-agora: ${err.message}`);
  console.error("");
  console.error("You can manually download the binary from:");
  console.error(`  https://github.com/${REPO}/releases/tag/${VERSION}`);
  process.exit(1);
});
