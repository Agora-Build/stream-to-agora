//! Codec-mode decision + the public publisher enums.
//!
//! The publisher enums are placeholder unit-variants in this task —
//! Task 11 replaces them with real variants holding the per-mode
//! publisher structs from Tasks 9 and 10.

use crate::ffmpeg::MediaInfo;

/// Which Agora sender path we use. Picked once on startup based on the
/// input's codec ids — see `decide()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecMode {
    /// Both streams use codecs Agora's encoded-frame senders accept;
    /// ffmpeg runs with `-c copy` (demux only).
    Encoded,
    /// At least one stream's codec isn't in Agora's encoded set;
    /// ffmpeg decodes to raw YUV+PCM and we push via the raw senders.
    Raw,
}

/// The video codec names ffprobe reports for codecs we can publish via
/// `agora_video_encoded_image_sender_send` **AND** for which our current
/// ffmpeg pipeline output format (`-f h264`, Annex-B) is correct.
///
/// Passthrough video codecs: H.264 (`-f h264`) and H.265/HEVC
/// (`-f hevc`), both Annex-B, parsed by `parse::h264` / `parse::hevc`.
/// Anything else (VP8/VP9/AV1/mjpeg/MPEG-2/…) falls through to the Raw
/// path and is decoded → yuv420p, which works for every codec ffmpeg can
/// decode.
const VIDEO_ENCODED_OK: &[&str] = &["h264", "hevc"];

/// Passthrough audio codecs: AAC (`-f adts`) and Opus (`-f ogg`, then
/// de-Ogg'd by `parse::opus`). Everything else (PCMA/PCMU/G.722/MP3/…)
/// falls through to the Raw path and is decoded → s16le, which works for
/// every codec ffmpeg can decode.
const AUDIO_ENCODED_OK: &[&str] = &["aac", "opus"];

/// Pure mode-decision. Encoded if *both* present streams use accepted
/// codecs; otherwise Raw. A video-only or audio-only input takes only
/// the present stream's verdict.
pub fn decide(info: &MediaInfo) -> CodecMode {
    // Escape hatch: `STA_FORCE_MODE=raw|encoded` overrides the codec-based
    // decision. Useful for forcing the decode→raw path on an H.264/AAC
    // source (e.g. to sidestep a multi-slice bitstream) without re-encoding.
    if let Ok(s) = std::env::var("STA_FORCE_MODE") {
        match s.to_ascii_lowercase().as_str() {
            "raw" => return CodecMode::Raw,
            "encoded" => return CodecMode::Encoded,
            _ => {}
        }
    }
    let video_ok = match &info.video {
        Some(v) => VIDEO_ENCODED_OK.contains(&v.codec_name.as_str()),
        None => true, // no video — neutral
    };
    let audio_ok = match &info.audio {
        Some(a) => AUDIO_ENCODED_OK.contains(&a.codec_name.as_str()),
        None => true,
    };
    if video_ok && audio_ok { CodecMode::Encoded } else { CodecMode::Raw }
}

use std::os::raw::c_void;

use super::audio::{create_encoded as create_audio_encoded, create_raw as create_audio_raw,
                   EncodedAudioPublisher, RawAudioPublisher};
use super::error::AgoraError;
use super::video::{create_encoded as create_video_encoded, create_raw as create_video_raw,
                   EncodedVideoPublisher, RawVideoPublisher};

pub enum AudioPublisher {
    Encoded(EncodedAudioPublisher),
    Raw(RawAudioPublisher),
}

pub enum VideoPublisher {
    Encoded(EncodedVideoPublisher),
    Raw(RawVideoPublisher),
}

impl AudioPublisher {
    pub fn publish(&self) -> Result<(), AgoraError> {
        match self {
            AudioPublisher::Encoded(p) => p.publish(),
            AudioPublisher::Raw(p) => p.publish(),
        }
    }
    pub fn unpublish(&self) -> Result<(), AgoraError> {
        match self {
            AudioPublisher::Encoded(p) => p.unpublish(),
            AudioPublisher::Raw(p) => p.unpublish(),
        }
    }
}

impl VideoPublisher {
    pub fn publish(&self) -> Result<(), AgoraError> {
        match self {
            VideoPublisher::Encoded(p) => p.publish(),
            VideoPublisher::Raw(p) => p.publish(),
        }
    }
    pub fn unpublish(&self) -> Result<(), AgoraError> {
        match self {
            VideoPublisher::Encoded(p) => p.unpublish(),
            VideoPublisher::Raw(p) => p.unpublish(),
        }
    }
}

/// Dispatch helper invoked from `Session::create_audio_publisher`.
/// `codec_name` is ffprobe's audio codec id (e.g. "aac", "opus"); used
/// only on the Encoded path.
pub(super) fn create_audio(
    svc: *mut c_void,
    conn: *mut c_void,
    factory: *mut c_void,
    mode: CodecMode,
    codec_name: &str,
) -> Result<AudioPublisher, AgoraError> {
    match mode {
        CodecMode::Encoded => {
            create_audio_encoded(svc, conn, factory, codec_name).map(AudioPublisher::Encoded)
        }
        CodecMode::Raw => create_audio_raw(svc, conn, factory).map(AudioPublisher::Raw),
    }
}

/// Dispatch helper invoked from `Session::create_video_publisher`.
/// `codec_name` is ffprobe's video codec id (e.g. "h264", "hevc"); used
/// only on the Encoded path.
pub(super) fn create_video(
    svc: *mut c_void,
    conn: *mut c_void,
    factory: *mut c_void,
    mode: CodecMode,
    codec_name: &str,
) -> Result<VideoPublisher, AgoraError> {
    match mode {
        CodecMode::Encoded => {
            create_video_encoded(svc, conn, factory, codec_name).map(VideoPublisher::Encoded)
        }
        CodecMode::Raw => create_video_raw(svc, conn, factory).map(VideoPublisher::Raw),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ffmpeg::probe::parse_probe_json;

    const H264_AAC: &[u8] = include_bytes!("../../tests/fixtures/probe-h264-aac.json");
    const VP9_OPUS: &[u8] = include_bytes!("../../tests/fixtures/probe-vp9-opus.json");
    const MPEG2_MP3: &[u8] = include_bytes!("../../tests/fixtures/probe-mpeg2-mp3.json");
    const HEVC_OPUS: &[u8] = include_bytes!("../../tests/fixtures/probe-hevc-opus.json");

    fn info(json: &[u8]) -> MediaInfo {
        parse_probe_json(json).unwrap()
    }

    #[test]
    fn h264_aac_picks_encoded()  { assert_eq!(decide(&info(H264_AAC)),  CodecMode::Encoded); }
    #[test]
    fn vp9_opus_picks_raw()      { assert_eq!(decide(&info(VP9_OPUS)),  CodecMode::Raw); }
    #[test]
    fn mpeg2_mp3_picks_raw()     { assert_eq!(decide(&info(MPEG2_MP3)), CodecMode::Raw); }

    #[test]
    fn video_only_input_uses_video_verdict() {
        let mut i = info(H264_AAC);
        i.audio = None;
        assert_eq!(decide(&i), CodecMode::Encoded);
        let mut i = info(MPEG2_MP3);
        i.audio = None;
        assert_eq!(decide(&i), CodecMode::Raw);
    }

    #[test]
    fn audio_unsupported_forces_raw_even_if_video_ok() {
        let mut i = info(H264_AAC);
        i.audio.as_mut().unwrap().codec_name = "flac".into();
        assert_eq!(decide(&i), CodecMode::Raw);
    }

    #[test]
    fn hevc_and_opus_combinations_pick_encoded() {
        // h264+opus, hevc+aac, hevc+opus all pass the widened allowlists.
        for (v, a) in [("h264", "opus"), ("hevc", "aac"), ("hevc", "opus")] {
            let mut i = info(H264_AAC);
            i.video.as_mut().unwrap().codec_name = v.into();
            i.audio.as_mut().unwrap().codec_name = a.into();
            assert_eq!(decide(&i), CodecMode::Encoded, "{v}+{a} should be Encoded");
        }
    }

    #[test]
    fn hevc_video_only_picks_encoded() {
        let mut i = info(H264_AAC);
        i.video.as_mut().unwrap().codec_name = "hevc".into();
        i.audio = None;
        assert_eq!(decide(&i), CodecMode::Encoded);
    }

    #[test]
    fn vp9_with_opus_still_raw_because_video_unsupported() {
        // Opus is now allowed, but VP9 is not → still Raw.
        assert_eq!(decide(&info(VP9_OPUS)), CodecMode::Raw);
    }

    #[test]
    fn real_hevc_opus_fixture_probe_picks_encoded() {
        // Parsed from the actual ffprobe JSON of tests/fixtures/hevc-opus-5s.mp4.
        let i = info(HEVC_OPUS);
        assert_eq!(i.video.as_ref().unwrap().codec_name, "hevc");
        assert_eq!(i.audio.as_ref().unwrap().codec_name, "opus");
        assert_eq!(decide(&i), CodecMode::Encoded);
    }
}
