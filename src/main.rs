//! stream-to-agora — push local files (and later https/rtmp/rtsp) to
//! an Agora RTC channel.
//!
//! Phase 0 (this commit): CLI surface + arg validation, no SDK yet.
//! Phase 1: connect to channel, log "ready" — proves SDK + token + FFI.
//! Phase 2: stream a static H.264/AAC test file.
//! Phase 3: arbitrary file via ffmpeg pipeline.
//! Phase 4: https / rtmp / rtsp inputs.

use anyhow::{Result, bail};
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "stream-to-agora",
    version,
    about = "Stream a local file (or later https/rtmp/rtsp) to an Agora RTC channel.",
    long_about = "Decodes the input via ffmpeg, pushes raw YUV/PCM frames to an \
                  Agora RTC channel as the given user. The caller supplies a \
                  pre-minted RTC token — minting tokens is intentionally not \
                  this tool's job. Use `atem token …` or your token service."
)]
struct Cli {
    /// Input source: local file path. Later releases also accept
    /// https://, rtmp://, rtsp:// URLs (ffmpeg handles all of those
    /// natively, so the input arg shape stays the same).
    input: String,

    /// Agora App ID. Falls back to AGORA_APP_ID env var.
    #[arg(long, env = "AGORA_APP_ID")]
    app_id: String,

    /// RTC channel name.
    #[arg(long)]
    channel: String,

    /// RTC user identifier. All-digit → int uid. Non-digit → string
    /// account. Leading "s/" forces string mode (e.g. "s/1232").
    /// Same parsing convention as `atem serv rtc`.
    #[arg(long = "rtc-user-id")]
    rtc_user_id: String,

    /// Pre-minted RTC token. Required — mint via `atem token …` or
    /// your token service.
    #[arg(long)]
    token: String,

    /// Loop the input forever. Useful for steady-state load testing.
    #[arg(long)]
    r#loop: bool,

    /// Push only the audio track. Mutually exclusive with --video-only.
    #[arg(long)]
    audio_only: bool,

    /// Push only the video track. Mutually exclusive with --audio-only.
    #[arg(long)]
    video_only: bool,

    /// Path to the ffmpeg binary. Defaults to "ffmpeg" on PATH.
    #[arg(long, default_value = "ffmpeg")]
    ffmpeg_path: String,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    if cli.audio_only && cli.video_only {
        bail!("--audio-only and --video-only are mutually exclusive");
    }

    let input_kind = classify_input(&cli.input);
    if matches!(input_kind, InputKind::LocalFile) {
        let path = PathBuf::from(&cli.input);
        if !path.exists() {
            bail!("Input file does not exist: {}", path.display());
        }
    } else {
        bail!(
            "Input '{}' looks like a {:?}; only local files are supported in v0.1. \
             Coming in a later release.", cli.input, input_kind,
        );
    }

    println!("stream-to-agora (v0.1 — Phase 0 scaffold, no SDK yet)");
    println!("  app id:       {}", redact_tail(&cli.app_id));
    println!("  channel:      {}", cli.channel);
    println!("  rtc user:     {}", cli.rtc_user_id);
    println!("  input:        {}  ({:?})", cli.input, input_kind);
    println!("  ffmpeg path:  {}", cli.ffmpeg_path);
    println!("  loop:         {}", cli.r#loop);
    println!(
        "  tracks:       {}",
        match (cli.audio_only, cli.video_only) {
            (true, false)  => "audio only",
            (false, true)  => "video only",
            _              => "audio + video",
        }
    );
    println!();
    println!("Phase 1 will:");
    println!("  • Load the Agora RTC SDK (flat C API) via extern \"C\" FFI");
    println!("  • Join `{}` as `{}`", cli.channel, cli.rtc_user_id);
    println!("  • Log \"ready\" then idle until SIGINT");
    println!();
    println!("Not implemented yet — this is the v0.1 scaffold.");
    Ok(())
}

#[derive(Debug)]
enum InputKind {
    LocalFile,
    Https,
    Rtmp,
    Rtsp,
    Unknown,
}

fn classify_input(s: &str) -> InputKind {
    let lower = s.to_ascii_lowercase();
    if lower.starts_with("https://") || lower.starts_with("http://") { InputKind::Https }
    else if lower.starts_with("rtmp://")  { InputKind::Rtmp }
    else if lower.starts_with("rtsp://")  { InputKind::Rtsp }
    else if lower.contains("://")         { InputKind::Unknown }
    else                                   { InputKind::LocalFile }
}

/// Show the first 8 chars of an app id followed by ellipsis. Logs go
/// to stdout so don't leak the full id casually.
fn redact_tail(s: &str) -> String {
    if s.len() <= 12 { return s.to_string(); }
    format!("{}…", &s[..12])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_input_picks_right_kind() {
        assert!(matches!(classify_input("/tmp/x.mp4"), InputKind::LocalFile));
        assert!(matches!(classify_input("./x.mp4"),    InputKind::LocalFile));
        assert!(matches!(classify_input("https://example.com/x.mp4"), InputKind::Https));
        assert!(matches!(classify_input("HTTPS://example.com/x"),     InputKind::Https));
        assert!(matches!(classify_input("rtmp://example.com/live"),   InputKind::Rtmp));
        assert!(matches!(classify_input("rtsp://cam/stream"),         InputKind::Rtsp));
        assert!(matches!(classify_input("ftp://example.com/x"),       InputKind::Unknown));
    }

    #[test]
    fn redact_tail_keeps_short_strings_whole() {
        assert_eq!(redact_tail("short"), "short");
        assert_eq!(redact_tail("aaaaaaaaaaaa"), "aaaaaaaaaaaa"); // exactly 12
        assert_eq!(redact_tail("0123456789abcdef"), "0123456789ab…");
    }
}
