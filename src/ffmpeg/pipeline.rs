//! Spawn ffmpeg subprocess(es) and expose their stdout pipes as
//! `tokio::process::ChildStdout` readers. One ffmpeg child per output
//! stream (video, audio).

use anyhow::{Context, Result};
use std::path::Path;
use std::process::Stdio;
use tokio::process::{Child, ChildStdout, Command};

use crate::agora::CodecMode;
use crate::ffmpeg::MediaInfo;

/// Lite mirror of main.rs's `InputKind` — duplicated here so pipeline.rs
/// doesn't depend on main.rs's CLI module. Carries only what `input_args`
/// and `PipelineStream::respawn` need.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputKindLite { LocalFile, Http, Rtmp, Rtsp }

impl InputKindLite {
    pub fn is_remote(self) -> bool {
        matches!(self, InputKindLite::Http | InputKindLite::Rtmp | InputKindLite::Rtsp)
    }
}

/// Per-stream pipeline options. Shared across the video and audio
/// Children for one input.
#[derive(Debug, Clone, Default)]
pub struct PipelineOpts {
    pub http_headers: Vec<String>,
    pub user_agent: Option<String>,
    pub rtsp_transport: Option<String>,
    pub reconnect_attempts: u32,
    pub loop_forever: bool,
}

impl PipelineOpts {
    pub fn with_loop(mut self, l: bool) -> Self { self.loop_forever = l; self }
}

/// Pure input-side argument builder. Returns the args that go BEFORE
/// `-i <url>` in the ffmpeg invocation.
pub fn input_args(kind: InputKindLite, opts: &PipelineOpts) -> Vec<String> {
    let mut a = Vec::new();
    if opts.loop_forever {
        a.push("-stream_loop".into());
        a.push("-1".into());
    }
    match kind {
        InputKindLite::Http => {
            a.push("-reconnect".into()); a.push("1".into());
            a.push("-reconnect_streamed".into()); a.push("1".into());
            a.push("-reconnect_delay_max".into()); a.push("5".into());
            if !opts.http_headers.is_empty() {
                let mut joined = String::new();
                for h in &opts.http_headers {
                    joined.push_str(h);
                    joined.push_str("\r\n");
                }
                a.push("-headers".into());
                a.push(joined);
            }
            if let Some(ua) = &opts.user_agent {
                a.push("-user_agent".into());
                a.push(ua.clone());
            }
        }
        InputKindLite::Rtsp => {
            if let Some(t) = &opts.rtsp_transport {
                a.push("-rtsp_transport".into());
                a.push(t.clone());
            }
        }
        InputKindLite::Rtmp | InputKindLite::LocalFile => {}
    }
    a
}

/// One ffmpeg subprocess for one direction (video or audio). Owns the
/// Child plus enough state to rebuild it on demand (for RTMP/RTSP
/// reconnect in P3-T6's pump tasks).
pub struct PipelineStream {
    pub child: Child,
    ffmpeg_bin: std::path::PathBuf,
    input: std::path::PathBuf,
    kind: InputKindLite,
    opts: PipelineOpts,
    /// Output args (the ones AFTER `-i <url>`) — codec-mode-specific.
    output_args: Vec<String>,
}

impl PipelineStream {
    pub fn stdout(&mut self) -> Option<&mut ChildStdout> {
        self.child.stdout.as_mut()
    }

    pub fn kind(&self) -> InputKindLite { self.kind }

    /// Kill the current child (SIGKILL via kill_on_drop semantics; we
    /// also call `start_kill` to ask the runtime to send the signal
    /// immediately) and spawn a fresh ffmpeg with the same input + opts
    /// + output args. Replaces `self.child`. Returns when the new child
    /// has been spawned (the new stdout pipe is ready for reads).
    pub async fn respawn(&mut self) -> anyhow::Result<()> {
        let _ = self.child.start_kill();
        let new_child = spawn_one(
            &self.ffmpeg_bin, &self.input, self.kind, &self.opts, &self.output_args,
        )?;
        self.child = new_child;
        Ok(())
    }
}

pub struct Pipeline {
    pub video: Option<PipelineStream>,
    pub audio: Option<PipelineStream>,
}

impl Pipeline {
    /// Spawn ffmpeg(s) appropriate to the chosen `CodecMode`.
    ///
    /// Encoded mode: `-c copy` for both streams, output formats `h264`
    /// (Annex-B) for video and `adts` (AAC ADTS) for audio.
    ///
    /// Raw mode: video → `-pix_fmt yuv420p -f rawvideo`; audio →
    /// `-acodec pcm_s16le -ar 48000 -ac <input_channel_count> -f s16le`.
    pub fn spawn(
        input: &Path,
        ffmpeg_bin: &Path,
        info: &MediaInfo,
        mode: CodecMode,
        kind: InputKindLite,
        opts: &PipelineOpts,
    ) -> Result<Self> {
        let video = info.video.as_ref().map(|_v| -> Result<PipelineStream> {
            let output_args = video_args(mode);
            let child = spawn_one(ffmpeg_bin, input, kind, opts, &output_args)?;
            Ok(PipelineStream {
                child,
                ffmpeg_bin: ffmpeg_bin.to_path_buf(),
                input: input.to_path_buf(),
                kind, opts: opts.clone(), output_args,
            })
        }).transpose()?;
        let audio = info.audio.as_ref().map(|a| -> Result<PipelineStream> {
            let ac = a.channels.unwrap_or(2).max(1);
            let output_args = audio_args(mode, ac);
            let child = spawn_one(ffmpeg_bin, input, kind, opts, &output_args)?;
            Ok(PipelineStream {
                child,
                ffmpeg_bin: ffmpeg_bin.to_path_buf(),
                input: input.to_path_buf(),
                kind, opts: opts.clone(), output_args,
            })
        }).transpose()?;
        Ok(Pipeline { video, audio })
    }

    pub fn video_stdout(&mut self) -> Option<&mut ChildStdout> {
        self.video.as_mut().and_then(|s| s.stdout())
    }
    pub fn audio_stdout(&mut self) -> Option<&mut ChildStdout> {
        self.audio.as_mut().and_then(|s| s.stdout())
    }
}

fn video_args(mode: CodecMode) -> Vec<String> {
    match mode {
        CodecMode::Encoded => vec![
            "-map".into(), "0:v:0".into(),
            "-c:v".into(), "copy".into(),
            "-an".into(),
            // `-f h264` auto-applies the h264_mp4toannexb BSF which
            // inserts SPS+PPS into the bitstream at file start. No
            // extra BSF — dump_extra=freq=keyframe rejects mp4-sourced
            // copy streams with "Invalid data found" and yields an
            // empty pipe.
            "-f".into(), "h264".into(),
            "pipe:1".into(),
        ],
        CodecMode::Raw => vec![
            "-map".into(), "0:v:0".into(),
            "-an".into(),
            "-pix_fmt".into(), "yuv420p".into(),
            "-f".into(), "rawvideo".into(),
            "pipe:1".into(),
        ],
    }
}

fn audio_args(mode: CodecMode, channels: u32) -> Vec<String> {
    match mode {
        CodecMode::Encoded => vec![
            "-map".into(), "0:a:0".into(),
            "-c:a".into(), "copy".into(),
            "-vn".into(),
            "-f".into(), "adts".into(),
            "pipe:1".into(),
        ],
        CodecMode::Raw => vec![
            "-map".into(), "0:a:0".into(),
            "-vn".into(),
            "-acodec".into(), "pcm_s16le".into(),
            "-ar".into(), "48000".into(),
            "-ac".into(), channels.to_string(),
            "-f".into(), "s16le".into(),
            "pipe:1".into(),
        ],
    }
}

fn spawn_one(
    ffmpeg_bin: &Path,
    input: &Path,
    kind: InputKindLite,
    opts: &PipelineOpts,
    output_args: &[String],
) -> Result<Child> {
    let mut cmd = Command::new(ffmpeg_bin);
    cmd.arg("-hide_banner").arg("-loglevel").arg("error");
    for a in input_args(kind, opts) {
        cmd.arg(a);
    }
    cmd.arg("-i").arg(input);
    for a in output_args { cmd.arg(a); }
    cmd.stdout(Stdio::piped())
       .stderr(Stdio::piped())
       .stdin(Stdio::null());
    cmd.kill_on_drop(true);
    cmd.spawn().with_context(|| format!("failed to spawn {}", ffmpeg_bin.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tokio::io::AsyncReadExt;

    fn fixture() -> PathBuf { PathBuf::from("tests/fixtures/loop-3s.mp4") }
    fn ffmpeg() -> PathBuf {
        std::env::var_os("FFMPEG")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/usr/bin/ffmpeg"))
    }

    fn opts(headers: &[&str], ua: Option<&str>, rtsp: Option<&str>, attempts: u32) -> PipelineOpts {
        PipelineOpts {
            http_headers: headers.iter().map(|s| s.to_string()).collect(),
            user_agent: ua.map(String::from),
            rtsp_transport: rtsp.map(String::from),
            reconnect_attempts: attempts,
            loop_forever: false,
        }
    }

    #[test]
    fn input_args_local_file_no_extras() {
        let a = input_args(InputKindLite::LocalFile, &opts(&[], None, None, 0));
        assert!(!a.iter().any(|s| s == "-reconnect"));
        assert!(!a.iter().any(|s| s == "-headers"));
        assert!(!a.iter().any(|s| s == "-rtsp_transport"));
    }

    #[test]
    fn input_args_http_gets_reconnect_triple() {
        let a = input_args(InputKindLite::Http, &opts(&[], None, None, 0));
        assert!(a.windows(2).any(|w| w == ["-reconnect", "1"]));
        assert!(a.windows(2).any(|w| w == ["-reconnect_streamed", "1"]));
        assert!(a.windows(2).any(|w| w == ["-reconnect_delay_max", "5"]));
    }

    #[test]
    fn input_args_http_with_headers_and_ua() {
        let a = input_args(
            InputKindLite::Http,
            &opts(&["Authorization: Bearer x", "X-Custom: y"], Some("MyAgent/1.0"), None, 0),
        );
        let hdr_idx = a.iter().position(|s| s == "-headers").unwrap();
        assert_eq!(a[hdr_idx + 1], "Authorization: Bearer x\r\nX-Custom: y\r\n");
        let ua_idx = a.iter().position(|s| s == "-user_agent").unwrap();
        assert_eq!(a[ua_idx + 1], "MyAgent/1.0");
    }

    #[test]
    fn input_args_rtsp_transport() {
        let a = input_args(InputKindLite::Rtsp, &opts(&[], None, Some("tcp"), 0));
        assert!(a.windows(2).any(|w| w == ["-rtsp_transport", "tcp"]));
    }

    #[test]
    fn input_args_rtmp_has_no_http_extras() {
        let a = input_args(InputKindLite::Rtmp, &opts(&["X: y"], Some("UA"), None, 0));
        assert!(!a.iter().any(|s| s == "-headers"));
        assert!(!a.iter().any(|s| s == "-user_agent"));
        assert!(!a.iter().any(|s| s == "-reconnect"));
    }

    #[test]
    fn input_args_loop_forever_emits_stream_loop() {
        let a = input_args(InputKindLite::LocalFile, &opts(&[], None, None, 0).with_loop(true));
        assert!(a.windows(2).any(|w| w == ["-stream_loop", "-1"]));
    }

    #[tokio::test]
    async fn encoded_mode_video_pipe_has_h264_annexb_data() {
        let info = crate::ffmpeg::probe(&fixture(), None).unwrap();
        let opts = PipelineOpts::default();
        let mut pipe = Pipeline::spawn(
            &fixture(), &ffmpeg(), &info, CodecMode::Encoded,
            InputKindLite::LocalFile, &opts,
        ).unwrap();
        let mut buf = [0u8; 1024];
        let n = pipe.video_stdout().unwrap().read(&mut buf).await.unwrap();
        assert!(n > 4, "expected H.264 NALUs; got {} bytes", n);
        // First 4 bytes should be the Annex-B start code 00 00 00 01.
        assert_eq!(&buf[..4], &[0, 0, 0, 1], "first 4 bytes should be Annex-B start code");
    }

    #[tokio::test]
    async fn raw_mode_video_pipe_emits_yuv_bytes() {
        let info = crate::ffmpeg::probe(&fixture(), None).unwrap();
        let (w, h) = {
            let v = info.video.as_ref().expect("fixture has video stream");
            (v.width.expect("width"), v.height.expect("height"))
        };
        let opts = PipelineOpts::default();
        let mut pipe = Pipeline::spawn(
            &fixture(), &ffmpeg(), &info, CodecMode::Raw,
            InputKindLite::LocalFile, &opts,
        ).unwrap();
        let need = crate::parse::yuv::frame_bytes(w, h);
        let mut filled = 0;
        let mut buf = vec![0u8; need];
        while filled < need {
            let n = pipe.video_stdout().unwrap().read(&mut buf[filled..]).await.unwrap();
            if n == 0 { break; }
            filled += n;
        }
        assert_eq!(filled, need, "expected one full yuv420p {w}x{h} frame");
    }
}
