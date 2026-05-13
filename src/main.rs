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
mod ffmpeg;
mod parse;

use anyhow::{Result, bail};
use clap::Parser;
use std::path::PathBuf;
use tokio::io::AsyncReadExt;

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

    if cli.audio_only || cli.video_only {
        anyhow::bail!("--audio-only and --video-only are not yet implemented in Phase 2 (planned for Phase 3 polish).");
    }

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

    // ── Probe input, decide encoded vs raw, spawn ffmpeg pipeline ─────
    let ffmpeg_bin = std::path::PathBuf::from(&cli.ffmpeg_path);
    let ffprobe_bin = ffmpeg_bin.parent().map(|d| d.join("ffprobe"));
    let info = ffmpeg::probe(std::path::Path::new(&cli.input), ffprobe_bin.as_deref())
        .map_err(|e| anyhow::anyhow!("probe failed: {e}"))?;
    if info.video.is_none() {
        anyhow::bail!("Input has no video stream; Phase 2 requires one (audio-only publish coming in Phase 3).");
    }
    if info.audio.is_none() {
        eprintln!("warning: input has no audio stream — publishing video only.");
    }
    let mode = agora::decide(&info);
    eprintln!("  codec mode: {mode:?}");

    // ── Create publishers and apply video metadata ────────────────────
    let mut audio_pub_opt = if info.audio.is_some() {
        Some(session.create_audio_publisher(mode)?)
    } else { None };
    let mut video_pub = session.create_video_publisher(mode)?;

    let v = info.video.as_ref().unwrap();
    let (w, h) = (v.width.unwrap_or(0), v.height.unwrap_or(0));
    let (fps_n, fps_d) = v.avg_frame_rate.unwrap_or((30, 1));
    match &mut video_pub {
        agora::VideoPublisher::Encoded(p) => p.set_metadata(w, h, fps_n, fps_d),
        agora::VideoPublisher::Raw(p) => p.set_dimensions(w, h),
    }

    // ── Publish ───────────────────────────────────────────────────────
    video_pub.publish()?;
    if let Some(ap) = audio_pub_opt.as_ref() { ap.publish()?; }

    // ── Spawn ffmpeg + frame pumps ────────────────────────────────────
    let mut pipeline = ffmpeg::Pipeline::spawn(
        std::path::Path::new(&cli.input), &ffmpeg_bin, &info, mode, cli.r#loop,
    )?;

    let session_start = std::time::Instant::now();
    let audio_channels = info.audio.as_ref().and_then(|a| a.channels).unwrap_or(2).max(1);
    let video_tx = session.sender();
    let audio_tx = session.sender();

    // Destructure publishers out so we can move concrete types into the pump tasks.
    let video_stdout = pipeline.video.take().and_then(|mut c| c.stdout.take());
    let audio_stdout = pipeline.audio.take().and_then(|mut c| c.stdout.take());

    match video_pub {
        agora::VideoPublisher::Encoded(vp) => {
            if let Some(vs) = video_stdout {
                tokio::spawn(pump_h264(vp, vs, session_start, fps_n, fps_d, video_tx));
            }
        }
        agora::VideoPublisher::Raw(vp) => {
            if let Some(vs) = video_stdout {
                tokio::spawn(pump_yuv(vp, vs, session_start, fps_n, fps_d, w, h, video_tx));
            }
        }
    }
    if let (Some(ap), Some(as_)) = (audio_pub_opt.take(), audio_stdout) {
        match ap {
            agora::AudioPublisher::Encoded(ap) => {
                tokio::spawn(pump_aac(ap, as_, audio_tx));
            }
            agora::AudioPublisher::Raw(ap) => {
                tokio::spawn(pump_pcm(ap, as_, audio_channels, audio_tx));
            }
        }
    }

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

async fn pump_h264(
    p: agora::video::EncodedVideoPublisher,
    mut stdout: tokio::process::ChildStdout,
    session_start: std::time::Instant,
    fps_n: u32, fps_d: u32,
    tx: std::sync::mpsc::Sender<agora::ConnEvent>,
) {
    let mut buf = Vec::with_capacity(256 * 1024);
    let mut tmp = vec![0u8; 64 * 1024];
    let mut frame_idx: u64 = 0;
    let fps_n = fps_n.max(1) as u64;
    let fps_d = fps_d.max(1) as u64;
    loop {
        let n = match stdout.read(&mut tmp).await {
            Ok(0) => {
                if let Ok(Some(au)) = parse::h264::next_au(&buf, true) {
                    let _ = p.push_h264(au.data, au.is_keyframe,
                        (frame_idx as i64) * 1000 * fps_d as i64 / fps_n as i64);
                }
                return;
            }
            Ok(n) => n,
            Err(e) => {
                let _ = tx.send(agora::ConnEvent::Failed { code: 0, msg: format!("ffmpeg video pipe: {e}") });
                return;
            }
        };
        buf.extend_from_slice(&tmp[..n]);

        loop {
            match parse::h264::next_au(&buf, false) {
                Ok(None) => break,
                Err(e) => {
                    let _ = tx.send(agora::ConnEvent::Failed { code: 0, msg: format!("parse h264: {e}") });
                    return;
                }
                Ok(Some(au)) => {
                    let au_len = au.data.len();
                    let is_keyframe = au.is_keyframe;
                    let target = session_start + std::time::Duration::from_micros(
                        frame_idx * 1_000_000 * fps_d / fps_n
                    );
                    tokio::time::sleep_until(tokio::time::Instant::from_std(target)).await;
                    let pts_ms = (frame_idx as i64) * 1000 * fps_d as i64 / fps_n as i64;
                    let push_result = p.push_h264(&buf[..au_len], is_keyframe, pts_ms);
                    if let Err(e) = push_result {
                        let _ = tx.send(agora::ConnEvent::Failed { code: e.code.unwrap_or(0), msg: e.to_string() });
                        return;
                    }
                    frame_idx += 1;
                    buf.drain(..au_len);
                }
            }
        }
    }
}

async fn pump_aac(
    p: agora::audio::EncodedAudioPublisher,
    mut stdout: tokio::process::ChildStdout,
    tx: std::sync::mpsc::Sender<agora::ConnEvent>,
) {
    let mut buf = Vec::with_capacity(64 * 1024);
    let mut tmp = vec![0u8; 8 * 1024];
    loop {
        let n = match stdout.read(&mut tmp).await {
            Ok(0) => return,
            Ok(n) => n,
            Err(e) => {
                let _ = tx.send(agora::ConnEvent::Failed { code: 0, msg: format!("ffmpeg audio pipe: {e}") });
                return;
            }
        };
        buf.extend_from_slice(&tmp[..n]);
        loop {
            match parse::aac::next_frame(&buf) {
                Ok(None) => break,
                Err(e) => {
                    let _ = tx.send(agora::ConnEvent::Failed { code: 0, msg: format!("parse aac: {e}") });
                    return;
                }
                Ok(Some(f)) => {
                    let len = f.data.len();
                    let sr = f.sample_rate;
                    let spc = f.samples_per_channel;
                    let ch = f.channels;
                    if let Err(e) = p.push_aac(&buf[..len], sr, spc, ch) {
                        let _ = tx.send(agora::ConnEvent::Failed { code: e.code.unwrap_or(0), msg: e.to_string() });
                        return;
                    }
                    buf.drain(..len);
                }
            }
        }
    }
}

async fn pump_yuv(
    p: agora::video::RawVideoPublisher,
    mut stdout: tokio::process::ChildStdout,
    session_start: std::time::Instant,
    fps_n: u32, fps_d: u32, w: u32, h: u32,
    tx: std::sync::mpsc::Sender<agora::ConnEvent>,
) {
    let need = parse::yuv::frame_bytes(w, h);
    let mut buf = vec![0u8; need];
    let mut frame_idx: u64 = 0;
    let fps_n = fps_n.max(1) as u64;
    let fps_d = fps_d.max(1) as u64;
    loop {
        let mut filled = 0;
        while filled < need {
            match stdout.read(&mut buf[filled..]).await {
                Ok(0) => return,
                Ok(n) => filled += n,
                Err(e) => {
                    let _ = tx.send(agora::ConnEvent::Failed { code: 0, msg: format!("ffmpeg video pipe: {e}") });
                    return;
                }
            }
        }
        let target = session_start + std::time::Duration::from_micros(
            frame_idx * 1_000_000 * fps_d / fps_n
        );
        tokio::time::sleep_until(tokio::time::Instant::from_std(target)).await;
        let pts_ms = (frame_idx as i64) * 1000 * fps_d as i64 / fps_n as i64;
        if let Err(e) = p.push_yuv420p(&buf, pts_ms) {
            let _ = tx.send(agora::ConnEvent::Failed { code: e.code.unwrap_or(0), msg: e.to_string() });
            return;
        }
        frame_idx += 1;
    }
}

async fn pump_pcm(
    p: agora::audio::RawAudioPublisher,
    mut stdout: tokio::process::ChildStdout,
    channels: u32,
    tx: std::sync::mpsc::Sender<agora::ConnEvent>,
) {
    let sample_rate = 48000u32;
    let samples_per_chunk = sample_rate / 100; // 10 ms
    let need = parse::pcm::frame_bytes(samples_per_chunk, channels);
    let mut buf = vec![0u8; need];
    let mut chunk_idx: u64 = 0;
    loop {
        let mut filled = 0;
        while filled < need {
            match stdout.read(&mut buf[filled..]).await {
                Ok(0) => return,
                Ok(n) => filled += n,
                Err(e) => {
                    let _ = tx.send(agora::ConnEvent::Failed { code: 0, msg: format!("ffmpeg audio pipe: {e}") });
                    return;
                }
            }
        }
        let ts_ms = (chunk_idx * 10) as u32;
        if let Err(e) = p.push_pcm(&buf, ts_ms, samples_per_chunk, channels, sample_rate) {
            let _ = tx.send(agora::ConnEvent::Failed { code: e.code.unwrap_or(0), msg: e.to_string() });
            return;
        }
        chunk_idx += 1;
    }
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
