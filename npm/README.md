# @agora-build/stream-to-agora

`stream-to-agora` — push local files (and later https/rtmp/rtsp) to an Agora RTC channel.

## Install

```bash
npm install -g @agora-build/stream-to-agora
```

The postinstall script downloads the right binary tarball for your platform from the GitHub release and unpacks the binary + the Agora SDK shared libraries it depends on into the package directory.

Supported: `linux-x64`, `linux-arm64`, `darwin-x64`, `darwin-arm64`.

## Alternative install (curl)

If GitHub is slow in your region:

```bash
curl -fsSL https://dl.agora.build/stream-to-agora/install.sh | bash
```

## Usage

See the [project README](https://github.com/Agora-Build/stream-to-agora#readme).
