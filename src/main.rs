//! stream-to-agora — push local files (and later https/rtmp/rtsp) to
//! an Agora RTC channel.
//!
//! Phase 0: CLI surface + arg validation.
//! Phase 1 (current): create the SDK service, open an RTC connection,
//!         log "ready", idle until SIGINT — proves SDK + token + FFI.
//! Phase 2: stream a static H.264/AAC test file.
//! Phase 3: arbitrary file via ffmpeg pipeline.
//! Phase 4: https / rtmp / rtsp inputs.

mod agora;

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

    /// Seconds to wait for the channel connection before giving up.
    #[arg(long, default_value_t = 10)]
    connect_timeout: u64,

    /// If set, hold the connection for this many seconds after `ready`,
    /// then disconnect and exit. Omit to idle until Ctrl-C.
    #[arg(long)]
    duration: Option<u64>,
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

    let uid = parse_rtc_user_id(&cli.rtc_user_id);

    eprintln!("stream-to-agora: connecting to channel `{}` as `{}`…", cli.channel, cli.rtc_user_id);
    let cfg = agora::SessionConfig {
        app_id: cli.app_id.clone(),
        channel: cli.channel.clone(),
        user_id: uid.value,
        use_string_uid: uid.string_mode,
        token: cli.token.clone(),
        connect_timeout: std::time::Duration::from_secs(cli.connect_timeout),
    };
    let session = agora::Session::connect(&cfg)?;

    println!("ready");
    eprintln!("  channel:   {}", cli.channel);
    eprintln!("  rtc user:  {}", cli.rtc_user_id);
    eprintln!("  conn id:   {}", session.conn_id);

    // Feed Shutdown into the session's channel on SIGINT or after --duration.
    let shutdown_tx = session.sender();
    {
        let tx = shutdown_tx.clone();
        ctrlc::set_handler(move || { let _ = tx.send(agora::ConnEvent::Shutdown); })
            .map_err(|e| anyhow::anyhow!("failed to install SIGINT handler: {e}"))?;
    }
    if let Some(secs) = cli.duration {
        let tx = shutdown_tx.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(secs));
            let _ = tx.send(agora::ConnEvent::Shutdown);
        });
        eprintln!("  holding for {secs}s, then disconnecting…");
    } else {
        eprintln!("  idling — press Ctrl-C to disconnect.");
    }

    session.run()?;            // returns Ok on Shutdown, Err on a fatal conn event
    eprintln!("disconnected.");
    drop(session);             // explicit: triggers clean SDK teardown
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

/// Parsed `--rtc-user-id`. `value` is what we pass to the SDK (always a
/// string — `user_id_t` is `const char*`); `string_mode` says whether the
/// SDK should treat the connection as a string-account connection.
/// Convention (matches `atem serv rtc`): all-digit → numeric; leading `s/`
/// or any non-digit → string account (with `s/` stripped if present).
#[derive(Debug)]
struct ParsedUid { value: String, string_mode: bool }

fn parse_rtc_user_id(raw: &str) -> ParsedUid {
    if let Some(rest) = raw.strip_prefix("s/") {
        return ParsedUid { value: rest.to_string(), string_mode: true };
    }
    let all_digits = !raw.is_empty() && raw.bytes().all(|b| b.is_ascii_digit());
    ParsedUid { value: raw.to_string(), string_mode: !all_digits }
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
    fn parse_rtc_user_id_modes() {
        // all digits -> numeric mode, passed through as the digit string
        let p = parse_rtc_user_id("42");
        assert_eq!(p.value, "42");
        assert!(!p.string_mode);
        // leading "s/" forces string mode, prefix stripped
        let p = parse_rtc_user_id("s/1232");
        assert_eq!(p.value, "1232");
        assert!(p.string_mode);
        // non-digit -> string mode, used verbatim
        let p = parse_rtc_user_id("alice");
        assert_eq!(p.value, "alice");
        assert!(p.string_mode);
        // "s/" with a non-digit body
        let p = parse_rtc_user_id("s/alice");
        assert_eq!(p.value, "alice");
        assert!(p.string_mode);
    }
}
