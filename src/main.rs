//! stream-to-agora — push local files (and later https/rtmp/rtsp) to
//! an Agora RTC channel.
//!
//! Phase 0: CLI surface + arg validation.
//! Phase 1: create the SDK service, open an RTC connection, log "ready", idle.
//! Phase 2: publish one audio + one video track from any file ffmpeg can read.
//! Phase 3 (current): remote sources (https/rtmp/rtsp); --audio-only/--video-only;
//!         --token-renew-cmd; ffmpeg input flags (--http-header, --user-agent,
//!         --rtsp-transport).

mod agora;
mod ffmpeg;
mod parse;

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

    /// Maximum reconnect attempts for RTMP/RTSP sources before giving up.
    /// HTTP/HTTPS reconnect is handled by ffmpeg's built-in `-reconnect`
    /// flags and isn't bounded by this counter. Set to 0 to disable
    /// respawn entirely.
    #[arg(long, default_value_t = 5)]
    reconnect_attempts: u32,

    /// Shell command that prints a fresh Agora RTC token on stdout when
    /// the SDK signals the current token is about to expire. The entire
    /// trimmed stdout becomes the new token. Supports `{channel}` and
    /// `{rtc_user_id}` placeholders that are substituted before
    /// running. Example:
    ///   atem token rtc create --channel {channel} --rtc-user-id {rtc_user_id} | awk '/^RTC Token/{getline; print; exit}'
    #[arg(long)]
    token_renew_cmd: Option<String>,

    /// HTTP header injected into ffmpeg's request when the input is an
    /// http(s):// URL. Repeatable: `--http-header 'Cookie: a=b'
    /// --http-header 'Authorization: Bearer …'`. Values must contain a
    /// colon and must not contain CR/LF.
    #[arg(long = "http-header", action = clap::ArgAction::Append)]
    http_header: Vec<String>,

    /// HTTP User-Agent string passed to ffmpeg for http(s):// inputs.
    #[arg(long)]
    user_agent: Option<String>,

    /// RTSP transport for rtsp:// inputs. UDP is the default (matches
    /// ffmpeg's default). Set to `tcp` for cameras behind a UDP-blocking
    /// NAT/firewall.
    #[arg(long, value_parser = ["tcp", "udp", "http"])]
    rtsp_transport: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    for h in &cli.http_header {
        if let Err(e) = validate_http_header(h) {
            anyhow::bail!(e);
        }
    }

    if cli.audio_only && cli.video_only {
        bail!("--audio-only and --video-only are mutually exclusive");
    }

    let input_kind = classify_input(&cli.input);
    match input_kind {
        InputKind::LocalFile => {
            let path = PathBuf::from(&cli.input);
            if !path.exists() {
                bail!("Input file does not exist: {}", path.display());
            }
        }
        InputKind::Https | InputKind::Rtmp | InputKind::Rtsp => {
            // remote sources flow through to ffmpeg
        }
        InputKind::Unknown => {
            bail!(
                "Input '{}' uses an unsupported URL scheme; \
                 supported: https://, http://, rtmp://, rtsp://, or a local file path.",
                cli.input,
            );
        }
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

    // If --token-renew-cmd is set, allocate a tokio mpsc channel that the
    // observer's on_token_privilege_will_expire trampoline will emit to.
    // The renew task consumes the receive end.
    let (renew_tx, renew_rx) = if cli.token_renew_cmd.is_some() {
        let (t, r) = tokio::sync::mpsc::unbounded_channel::<agora::ConnEvent>();
        (Some(t), Some(r))
    } else {
        (None, None)
    };

    let mut session = agora::Session::connect(&cfg).await?;

    // Wire the renew task's sender into the observer (P3-T4's RENEW_TX).
    // Must happen AFTER Session::connect because connect calls
    // observer::set_event_sender; we add ours on top.
    if let Some(rtx) = renew_tx {
        session.set_renew_sender(rtx);
    }

    println!("ready");
    eprintln!("  channel:   {}", cli.channel);
    eprintln!("  rtc user:  {}", cli.rtc_user_id);
    eprintln!("  conn id:   {}", session.conn_id);

    // ── Probe input, decide encoded vs raw, spawn ffmpeg pipeline ─────
    let ffmpeg_bin = std::path::PathBuf::from(&cli.ffmpeg_path);
    let ffprobe_bin = ffmpeg_bin.parent().map(|d| d.join("ffprobe"));
    let info = ffmpeg::probe(std::path::Path::new(&cli.input), ffprobe_bin.as_deref())
        .map_err(|e| anyhow::anyhow!("probe failed: {e}"))?;
    if cli.audio_only && info.audio.is_none() {
        anyhow::bail!("--audio-only requested but input has no audio stream");
    }
    if cli.video_only && info.video.is_none() {
        anyhow::bail!("--video-only requested but input has no video stream");
    }
    if !cli.audio_only && info.video.is_none() {
        anyhow::bail!("Input has no video stream; pass --audio-only to publish audio only");
    }
    if !cli.video_only && info.audio.is_none() {
        eprintln!("warning: input has no audio stream — publishing video only.");
    }
    let mode = agora::decide(&info);
    eprintln!("  codec mode: {mode:?}");

    // ── Create publishers and apply video metadata ────────────────────
    let mut audio_pub_opt = if info.audio.is_some() && !cli.video_only {
        Some(session.create_audio_publisher(mode)?)
    } else { None };
    let mut video_pub_opt = if info.video.is_some() && !cli.audio_only {
        Some(session.create_video_publisher(mode)?)
    } else { None };

    let (w, h, fps_n, fps_d) = match info.video.as_ref() {
        Some(v) => (
            v.width.unwrap_or(0),
            v.height.unwrap_or(0),
            v.avg_frame_rate.unwrap_or((30, 1)).0,
            v.avg_frame_rate.unwrap_or((30, 1)).1,
        ),
        None => (0, 0, 30, 1),
    };
    if let Some(vp) = video_pub_opt.as_mut() {
        match vp {
            agora::VideoPublisher::Encoded(p) => p.set_metadata(w, h, fps_n, fps_d),
            agora::VideoPublisher::Raw(p) => p.set_dimensions(w, h),
        }
    }

    // ── Publish ───────────────────────────────────────────────────────
    if let Some(vp) = video_pub_opt.as_ref() {
        vp.publish()?;
    }
    if let Some(ap) = audio_pub_opt.as_ref() { ap.publish()?; }

    // ── Spawn ffmpeg + frame pumps ────────────────────────────────────
    let kind_lite = match classify_input(&cli.input) {
        InputKind::LocalFile => ffmpeg::pipeline::InputKindLite::LocalFile,
        InputKind::Https => ffmpeg::pipeline::InputKindLite::Http,
        InputKind::Rtmp => ffmpeg::pipeline::InputKindLite::Rtmp,
        InputKind::Rtsp => ffmpeg::pipeline::InputKindLite::Rtsp,
        InputKind::Unknown => unreachable!("already rejected in startup gate"),
    };
    let pipe_opts = ffmpeg::pipeline::PipelineOpts {
        http_headers: cli.http_header.clone(),
        user_agent: cli.user_agent.clone(),
        rtsp_transport: cli.rtsp_transport.clone(),
        reconnect_attempts: cli.reconnect_attempts,
        loop_forever: cli.r#loop,
    };
    let pipeline_info = {
        let mut p = info.clone();
        if cli.video_only { p.audio = None; }
        if cli.audio_only { p.video = None; }
        p
    };
    let mut pipeline = ffmpeg::Pipeline::spawn(
        std::path::Path::new(&cli.input), &ffmpeg_bin, &pipeline_info, mode,
        kind_lite, &pipe_opts,
    )?;

    let session_start = std::time::Instant::now();
    let cancel = session.cancel_signal();
    let video_stream = pipeline.video.take();
    let audio_stream = pipeline.audio.take();
    let video_tx = session.sender();
    let audio_tx = session.sender();
    let audio_channels = info.audio.as_ref().and_then(|a| a.channels).unwrap_or(2).max(1);

    if let Some(vp) = video_pub_opt.take() {
        if let Some(vs) = video_stream {
            match vp {
                agora::VideoPublisher::Encoded(vp) => {
                    let jh = tokio::spawn(pump_h264(vp, vs, cancel.clone(), session_start, fps_n, fps_d, cli.reconnect_attempts, video_tx.clone()));
                    session.register_pump(jh).await;
                }
                agora::VideoPublisher::Raw(vp) => {
                    let jh = tokio::spawn(pump_yuv(vp, vs, cancel.clone(), session_start, fps_n, fps_d, w, h, cli.reconnect_attempts, video_tx.clone()));
                    session.register_pump(jh).await;
                }
            }
        }
    }
    if let Some(ap) = audio_pub_opt.take() {
        if let Some(as_) = audio_stream {
            match ap {
                agora::AudioPublisher::Encoded(ap) => {
                    let jh = tokio::spawn(pump_aac(ap, as_, cancel.clone(), cli.reconnect_attempts, audio_tx.clone()));
                    session.register_pump(jh).await;
                }
                agora::AudioPublisher::Raw(ap) => {
                    let jh = tokio::spawn(pump_pcm(ap, as_, cancel.clone(), audio_channels, cli.reconnect_attempts, audio_tx.clone()));
                    session.register_pump(jh).await;
                }
            }
        }
    }

    // Token-renew task: subscribe to TokenWillExpire on the renew channel,
    // run the shell command, take the entire trimmed stdout as the new
    // token, call RenewHandle::renew(). Registered with the Session so
    // Session::run's cancel+join handles its shutdown.
    if let (Some(cmd), Some(mut renew_rx)) = (cli.token_renew_cmd.clone(), renew_rx) {
        let renew_handle = session.renew_handle();
        let cancel_clone = session.cancel_signal();
        let chan = cli.channel.clone();
        let uid = cli.rtc_user_id.clone();
        let jh = tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    _ = cancel_clone.notified() => return,
                    ev = renew_rx.recv() => match ev {
                        Some(agora::ConnEvent::TokenWillExpire { current: _ }) => {
                            let sub = substitute_token_cmd(&cmd, &chan, &uid);
                            match tokio::process::Command::new("sh")
                                .args(["-c", &sub])
                                .output()
                                .await
                            {
                                Ok(o) if o.status.success() => {
                                    let new_token = String::from_utf8_lossy(&o.stdout)
                                        .trim()
                                        .to_string();
                                    if new_token.is_empty() {
                                        eprintln!("warning: token-renew-cmd produced empty stdout");
                                    } else if let Err(e) = renew_handle.renew(&new_token) {
                                        eprintln!("warning: agora_rtc_conn_renew_token failed: {e}");
                                    } else {
                                        eprintln!("token renewed");
                                    }
                                }
                                Ok(o) => eprintln!(
                                    "warning: token-renew-cmd exit {}: {}",
                                    o.status,
                                    String::from_utf8_lossy(&o.stderr).trim()
                                ),
                                Err(e) => eprintln!("warning: token-renew-cmd spawn failed: {e}"),
                            }
                        }
                        Some(_) => continue, // other events not for us
                        None => return,      // sender dropped — exit
                    }
                }
            }
        });
        session.register_pump(jh).await;
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

    session.run().await?;      // returns Ok on Shutdown, Err on a fatal conn event
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

/// Validate one `--http-header` value: must contain a `:`, must not
/// contain any CR or LF (RFC-7230 forbids them inside a header line).
fn validate_http_header(s: &str) -> Result<(), String> {
    if !s.contains(':') {
        return Err(format!("--http-header `{s}` missing colon (expected `Key: Value`)"));
    }
    if s.contains('\r') || s.contains('\n') {
        return Err(format!("--http-header `{s}` contains a line break (CRLF injection)"));
    }
    Ok(())
}

/// Substitute `{channel}` and `{rtc_user_id}` placeholders in the
/// `--token-renew-cmd` template.
fn substitute_token_cmd(template: &str, channel: &str, rtc_user_id: &str) -> String {
    template
        .replace("{channel}", channel)
        .replace("{rtc_user_id}", rtc_user_id)
}

async fn pump_h264(
    p: agora::video::EncodedVideoPublisher,
    mut stream: ffmpeg::pipeline::PipelineStream,
    cancel: std::sync::Arc<tokio::sync::Notify>,
    session_start: std::time::Instant,
    fps_n: u32, fps_d: u32,
    reconnect_attempts: u32,
    tx: tokio::sync::mpsc::UnboundedSender<agora::ConnEvent>,
) {
    use tokio::io::AsyncReadExt;
    let mut buf = Vec::with_capacity(256 * 1024);
    let mut tmp = vec![0u8; 64 * 1024];
    let mut frame_idx: u64 = 0;
    let fps_n = fps_n.max(1) as u64;
    let fps_d = fps_d.max(1) as u64;
    let mut consecutive_failures: u32 = 0;
    let mut last_respawn_at: Option<std::time::Instant> = None;
    let mut pushed_since_last_respawn: bool = true; // bootstrap: count as "made progress"

    loop {
        // Read one chunk OR cancellation.
        let n_or_eof = {
            let stdout = match stream.stdout() {
                Some(s) => s,
                None => return,
            };
            tokio::select! {
                biased;
                _ = cancel.notified() => return,
                r = stdout.read(&mut tmp) => r,
            }
        };
        match n_or_eof {
            Ok(0) => {
                // EOF. Respawn for remote sources if we have budget.
                if stream.kind().is_remote() && consecutive_failures < reconnect_attempts {
                    // Reset failure counter if we made progress recently.
                    if pushed_since_last_respawn {
                        consecutive_failures = 0;
                    }
                    eprintln!(
                        "video pump: source EOF; respawning ffmpeg (attempt {}/{})",
                        consecutive_failures + 1, reconnect_attempts,
                    );
                    if let Err(e) = stream.respawn().await {
                        let _ = tx.send(agora::ConnEvent::Failed {
                            code: 0,
                            msg: format!("video respawn failed: {e}"),
                        });
                        return;
                    }
                    last_respawn_at = Some(std::time::Instant::now());
                    consecutive_failures += 1;
                    pushed_since_last_respawn = false;
                    buf.clear(); // discard partial parser state across the seam
                    continue;
                }
                // No respawn — local file w/o loop, or exhausted budget.
                if consecutive_failures >= reconnect_attempts && stream.kind().is_remote() {
                    let _ = tx.send(agora::ConnEvent::Failed {
                        code: 0,
                        msg: format!(
                            "RTMP/RTSP source unreachable after {} attempts",
                            reconnect_attempts,
                        ),
                    });
                }
                return;
            }
            Ok(n) => {
                buf.extend_from_slice(&tmp[..n]);
            }
            Err(e) => {
                let _ = tx.send(agora::ConnEvent::Failed {
                    code: 0,
                    msg: format!("ffmpeg video pipe: {e}"),
                });
                return;
            }
        }

        // Drain frames.
        loop {
            match parse::h264::next_au(&buf, false) {
                Ok(None) => break,
                Err(e) => {
                    let _ = tx.send(agora::ConnEvent::Failed {
                        code: 0,
                        msg: format!("parse h264: {e}"),
                    });
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
                    if let Err(e) = p.push_h264(&buf[..au_len], is_keyframe, pts_ms) {
                        let _ = tx.send(agora::ConnEvent::Failed {
                            code: e.code.unwrap_or(0),
                            msg: e.to_string(),
                        });
                        return;
                    }
                    frame_idx += 1;
                    pushed_since_last_respawn = true;
                    // If 2s+ has passed since the last respawn and we
                    // just pushed, count it as progress and reset the
                    // consecutive_failures counter.
                    if let Some(t) = last_respawn_at {
                        if t.elapsed() >= std::time::Duration::from_secs(2) {
                            consecutive_failures = 0;
                            last_respawn_at = None;
                        }
                    }
                    buf.drain(..au_len);
                }
            }
        }
    }
}

async fn pump_aac(
    p: agora::audio::EncodedAudioPublisher,
    mut stream: ffmpeg::pipeline::PipelineStream,
    cancel: std::sync::Arc<tokio::sync::Notify>,
    reconnect_attempts: u32,
    tx: tokio::sync::mpsc::UnboundedSender<agora::ConnEvent>,
) {
    use tokio::io::AsyncReadExt;
    let mut buf = Vec::with_capacity(64 * 1024);
    let mut tmp = vec![0u8; 8 * 1024];
    let mut consecutive_failures: u32 = 0;
    let mut last_respawn_at: Option<std::time::Instant> = None;
    let mut pushed_since_last_respawn: bool = true;

    loop {
        let n_or_eof = {
            let stdout = match stream.stdout() {
                Some(s) => s,
                None => return,
            };
            tokio::select! {
                biased;
                _ = cancel.notified() => return,
                r = stdout.read(&mut tmp) => r,
            }
        };
        match n_or_eof {
            Ok(0) => {
                if stream.kind().is_remote() && consecutive_failures < reconnect_attempts {
                    if pushed_since_last_respawn { consecutive_failures = 0; }
                    eprintln!(
                        "audio pump: source EOF; respawning ffmpeg (attempt {}/{})",
                        consecutive_failures + 1, reconnect_attempts,
                    );
                    if let Err(e) = stream.respawn().await {
                        let _ = tx.send(agora::ConnEvent::Failed {
                            code: 0, msg: format!("audio respawn failed: {e}"),
                        });
                        return;
                    }
                    last_respawn_at = Some(std::time::Instant::now());
                    consecutive_failures += 1;
                    pushed_since_last_respawn = false;
                    buf.clear();
                    continue;
                }
                if consecutive_failures >= reconnect_attempts && stream.kind().is_remote() {
                    let _ = tx.send(agora::ConnEvent::Failed {
                        code: 0,
                        msg: format!("RTMP/RTSP source unreachable after {} attempts", reconnect_attempts),
                    });
                }
                return;
            }
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
            Err(e) => {
                let _ = tx.send(agora::ConnEvent::Failed {
                    code: 0, msg: format!("ffmpeg audio pipe: {e}"),
                });
                return;
            }
        }

        loop {
            match parse::aac::next_frame(&buf) {
                Ok(None) => break,
                Err(e) => {
                    let _ = tx.send(agora::ConnEvent::Failed {
                        code: 0, msg: format!("parse aac: {e}"),
                    });
                    return;
                }
                Ok(Some(f)) => {
                    let len = f.data.len();
                    let sr = f.sample_rate;
                    let spc = f.samples_per_channel;
                    let ch = f.channels;
                    if let Err(e) = p.push_aac(&buf[..len], sr, spc, ch) {
                        let _ = tx.send(agora::ConnEvent::Failed {
                            code: e.code.unwrap_or(0), msg: e.to_string(),
                        });
                        return;
                    }
                    pushed_since_last_respawn = true;
                    if let Some(t) = last_respawn_at {
                        if t.elapsed() >= std::time::Duration::from_secs(2) {
                            consecutive_failures = 0;
                            last_respawn_at = None;
                        }
                    }
                    buf.drain(..len);
                }
            }
        }
    }
}

async fn pump_yuv(
    p: agora::video::RawVideoPublisher,
    mut stream: ffmpeg::pipeline::PipelineStream,
    cancel: std::sync::Arc<tokio::sync::Notify>,
    session_start: std::time::Instant,
    fps_n: u32, fps_d: u32, w: u32, h: u32,
    reconnect_attempts: u32,
    tx: tokio::sync::mpsc::UnboundedSender<agora::ConnEvent>,
) {
    use tokio::io::AsyncReadExt;
    let need = parse::yuv::frame_bytes(w, h);
    let mut buf = vec![0u8; need];
    let mut frame_idx: u64 = 0;
    let fps_n = fps_n.max(1) as u64;
    let fps_d = fps_d.max(1) as u64;
    let mut consecutive_failures: u32 = 0;
    let mut last_respawn_at: Option<std::time::Instant> = None;
    let mut pushed_since_last_respawn: bool = true;

    loop {
        // Read a full frame.
        let mut filled = 0;
        let drained_eof: bool = loop {
            let stdout = match stream.stdout() {
                Some(s) => s,
                None => return,
            };
            let read_res = tokio::select! {
                biased;
                _ = cancel.notified() => return,
                r = stdout.read(&mut buf[filled..]) => r,
            };
            match read_res {
                Ok(0) => break true, // EOF mid-frame
                Ok(n) => {
                    filled += n;
                    if filled == need { break false; }
                }
                Err(e) => {
                    let _ = tx.send(agora::ConnEvent::Failed {
                        code: 0, msg: format!("ffmpeg video pipe: {e}"),
                    });
                    return;
                }
            }
        };

        if drained_eof {
            if stream.kind().is_remote() && consecutive_failures < reconnect_attempts {
                if pushed_since_last_respawn { consecutive_failures = 0; }
                eprintln!(
                    "video pump: source EOF; respawning ffmpeg (attempt {}/{})",
                    consecutive_failures + 1, reconnect_attempts,
                );
                if let Err(e) = stream.respawn().await {
                    let _ = tx.send(agora::ConnEvent::Failed {
                        code: 0, msg: format!("video respawn failed: {e}"),
                    });
                    return;
                }
                last_respawn_at = Some(std::time::Instant::now());
                consecutive_failures += 1;
                pushed_since_last_respawn = false;
                continue; // retry the read loop with the fresh stdout
            }
            if consecutive_failures >= reconnect_attempts && stream.kind().is_remote() {
                let _ = tx.send(agora::ConnEvent::Failed {
                    code: 0,
                    msg: format!("RTMP/RTSP source unreachable after {} attempts", reconnect_attempts),
                });
            }
            return;
        }

        let target = session_start + std::time::Duration::from_micros(
            frame_idx * 1_000_000 * fps_d / fps_n
        );
        tokio::time::sleep_until(tokio::time::Instant::from_std(target)).await;
        let pts_ms = (frame_idx as i64) * 1000 * fps_d as i64 / fps_n as i64;
        if let Err(e) = p.push_yuv420p(&buf, pts_ms) {
            let _ = tx.send(agora::ConnEvent::Failed {
                code: e.code.unwrap_or(0), msg: e.to_string(),
            });
            return;
        }
        frame_idx += 1;
        pushed_since_last_respawn = true;
        if let Some(t) = last_respawn_at {
            if t.elapsed() >= std::time::Duration::from_secs(2) {
                consecutive_failures = 0;
                last_respawn_at = None;
            }
        }
    }
}

async fn pump_pcm(
    p: agora::audio::RawAudioPublisher,
    mut stream: ffmpeg::pipeline::PipelineStream,
    cancel: std::sync::Arc<tokio::sync::Notify>,
    channels: u32,
    reconnect_attempts: u32,
    tx: tokio::sync::mpsc::UnboundedSender<agora::ConnEvent>,
) {
    use tokio::io::AsyncReadExt;
    let sample_rate = 48000u32;
    let samples_per_chunk = sample_rate / 100; // 10 ms
    let need = parse::pcm::frame_bytes(samples_per_chunk, channels);
    let mut buf = vec![0u8; need];
    let mut chunk_idx: u64 = 0;
    let mut consecutive_failures: u32 = 0;
    let mut last_respawn_at: Option<std::time::Instant> = None;
    let mut pushed_since_last_respawn: bool = true;

    loop {
        let mut filled = 0;
        let drained_eof: bool = loop {
            let stdout = match stream.stdout() {
                Some(s) => s,
                None => return,
            };
            let read_res = tokio::select! {
                biased;
                _ = cancel.notified() => return,
                r = stdout.read(&mut buf[filled..]) => r,
            };
            match read_res {
                Ok(0) => break true,
                Ok(n) => {
                    filled += n;
                    if filled == need { break false; }
                }
                Err(e) => {
                    let _ = tx.send(agora::ConnEvent::Failed {
                        code: 0, msg: format!("ffmpeg audio pipe: {e}"),
                    });
                    return;
                }
            }
        };

        if drained_eof {
            if stream.kind().is_remote() && consecutive_failures < reconnect_attempts {
                if pushed_since_last_respawn { consecutive_failures = 0; }
                eprintln!(
                    "audio pump: source EOF; respawning ffmpeg (attempt {}/{})",
                    consecutive_failures + 1, reconnect_attempts,
                );
                if let Err(e) = stream.respawn().await {
                    let _ = tx.send(agora::ConnEvent::Failed {
                        code: 0, msg: format!("audio respawn failed: {e}"),
                    });
                    return;
                }
                last_respawn_at = Some(std::time::Instant::now());
                consecutive_failures += 1;
                pushed_since_last_respawn = false;
                continue;
            }
            if consecutive_failures >= reconnect_attempts && stream.kind().is_remote() {
                let _ = tx.send(agora::ConnEvent::Failed {
                    code: 0,
                    msg: format!("RTMP/RTSP source unreachable after {} attempts", reconnect_attempts),
                });
            }
            return;
        }

        let ts_ms = (chunk_idx * 10) as u32;
        if let Err(e) = p.push_pcm(&buf, ts_ms, samples_per_chunk, channels, sample_rate) {
            let _ = tx.send(agora::ConnEvent::Failed {
                code: e.code.unwrap_or(0), msg: e.to_string(),
            });
            return;
        }
        chunk_idx += 1;
        pushed_since_last_respawn = true;
        if let Some(t) = last_respawn_at {
            if t.elapsed() >= std::time::Duration::from_secs(2) {
                consecutive_failures = 0;
                last_respawn_at = None;
            }
        }
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

    #[test]
    fn validate_http_header_ok() {
        assert!(validate_http_header("Authorization: Bearer eyJabc").is_ok());
        assert!(validate_http_header("X-Custom:value").is_ok());
    }

    #[test]
    fn validate_http_header_rejects_no_colon() {
        let e = validate_http_header("just-a-key").unwrap_err();
        assert!(e.contains("colon"));
    }

    #[test]
    fn validate_http_header_rejects_linebreaks() {
        let e = validate_http_header("X-Bad: foo\r\nInjection: bar").unwrap_err();
        assert!(e.contains("line break"));
        let e = validate_http_header("X-Bad: foo\nInjection: bar").unwrap_err();
        assert!(e.contains("line break"));
    }

    #[test]
    fn substitute_token_cmd_replaces_placeholders() {
        let s = substitute_token_cmd(
            "atem token rtc create --channel {channel} --rtc-user-id {rtc_user_id}",
            "demo", "42",
        );
        assert_eq!(s, "atem token rtc create --channel demo --rtc-user-id 42");
    }

    #[test]
    fn substitute_token_cmd_no_placeholders() {
        let s = substitute_token_cmd("plain command", "x", "y");
        assert_eq!(s, "plain command");
    }

    #[test]
    fn substitute_token_cmd_repeated_placeholder() {
        let s = substitute_token_cmd("{channel}-{channel}", "demo", "42");
        assert_eq!(s, "demo-demo");
    }
}
