//! Spawn ffmpeg subprocess(es) and expose their stdout pipes as
//! `tokio::process::ChildStdout` readers. One ffmpeg child per output
//! stream (video, audio).

use anyhow::{Context, Result};
use std::path::Path;
use std::process::Stdio;
use tokio::process::{Child, ChildStdout, Command};

use crate::agora::CodecMode;
use crate::ffmpeg::MediaInfo;

pub struct Pipeline {
    pub video: Option<Child>,
    pub audio: Option<Child>,
}

impl Pipeline {
    /// Spawn ffmpeg(s) appropriate to the chosen `CodecMode`.
    ///
    /// Encoded mode: `-c copy` for both streams, output formats `h264`
    /// (Annex-B) for video and `adts` (AAC ADTS) for audio.
    ///
    /// Raw mode: video → `-pix_fmt yuv420p -f rawvideo`; audio →
    /// `-acodec pcm_s16le -ar 48000 -ac <input_channel_count> -f s16le`.
    ///
    /// `loop_forever = true` sets `-stream_loop -1` on the input.
    pub fn spawn(
        input: &Path,
        ffmpeg_bin: &Path,
        info: &MediaInfo,
        mode: CodecMode,
        loop_forever: bool,
    ) -> Result<Self> {
        let video = info.video.as_ref().map(|_v| {
            spawn_one(ffmpeg_bin, input, loop_forever, &video_args(mode))
        }).transpose()?;
        let audio = info.audio.as_ref().map(|a| {
            let ac = a.channels.unwrap_or(2).max(1);
            spawn_one(ffmpeg_bin, input, loop_forever, &audio_args(mode, ac))
        }).transpose()?;
        Ok(Pipeline { video, audio })
    }

    pub fn video_stdout(&mut self) -> Option<&mut ChildStdout> {
        self.video.as_mut().and_then(|c| c.stdout.as_mut())
    }
    pub fn audio_stdout(&mut self) -> Option<&mut ChildStdout> {
        self.audio.as_mut().and_then(|c| c.stdout.as_mut())
    }
}

fn video_args(mode: CodecMode) -> Vec<String> {
    match mode {
        CodecMode::Encoded => vec![
            "-map".into(), "0:v:0".into(),
            "-c:v".into(), "copy".into(),
            "-an".into(),
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

fn spawn_one(ffmpeg_bin: &Path, input: &Path, loop_forever: bool, output_args: &[String])
    -> Result<Child>
{
    let mut cmd = Command::new(ffmpeg_bin);
    cmd.arg("-hide_banner").arg("-loglevel").arg("error");
    if loop_forever { cmd.arg("-stream_loop").arg("-1"); }
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

    #[tokio::test]
    async fn encoded_mode_video_pipe_has_h264_annexb_data() {
        let info = crate::ffmpeg::probe(&fixture(), None).unwrap();
        let mut pipe = Pipeline::spawn(&fixture(), &ffmpeg(), &info, CodecMode::Encoded, false).unwrap();
        let mut buf = [0u8; 1024];
        let n = pipe.video_stdout().unwrap().read(&mut buf).await.unwrap();
        assert!(n > 4, "expected H.264 NALUs; got {} bytes", n);
        // First 4 bytes should be the Annex-B start code 00 00 00 01.
        assert_eq!(&buf[..4], &[0, 0, 0, 1], "first 4 bytes should be Annex-B start code");
    }

    #[tokio::test]
    async fn raw_mode_video_pipe_emits_yuv_bytes() {
        let info = crate::ffmpeg::probe(&fixture(), None).unwrap();
        let mut pipe = Pipeline::spawn(&fixture(), &ffmpeg(), &info, CodecMode::Raw, false).unwrap();
        let need = crate::parse::yuv::frame_bytes(320, 180);
        let mut filled = 0;
        let mut buf = vec![0u8; need];
        while filled < need {
            let n = pipe.video_stdout().unwrap().read(&mut buf[filled..]).await.unwrap();
            if n == 0 { break; }
            filled += n;
        }
        assert_eq!(filled, need, "expected one full yuv420p 320x180 frame");
    }
}
