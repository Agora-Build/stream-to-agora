//! Codec-mode decision + the public publisher enums.
//!
//! `AudioPublisher`/`VideoPublisher` wrap the per-mode publisher structs
//! (`audio.rs`/`video.rs`) behind one `Encoded`/`Raw` enum so callers
//! don't branch on the mode `decide()` picked at startup.

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
/// the encoded-image sender **AND** for which our ffmpeg pipeline emits a
/// container the matching parser understands.
///
/// Passthrough video codecs: H.264/H.265 (Annex-B, `-f h264`/`-f hevc`,
/// `parse::h264`/`parse::hevc`) and VP8/VP9/AV1 (IVF, `-f ivf`,
/// `parse::ivf`). Anything else (mjpeg/MPEG-2/Theora/…) falls through to
/// the Raw path and is decoded → yuv420p, which works for every codec
/// ffmpeg can decode.
const VIDEO_ENCODED_OK: &[&str] = &["h264", "hevc", "vp8", "vp9", "av1"];

/// Passthrough audio codecs: AAC/HE-AAC/HE-AACv2 (`-f adts`,
/// `parse::aac`; profile picks the SDK codec id), Opus (`-f ogg`,
/// `parse::opus`), and G.711 µ-law/A-law (`-f mulaw`/`-f alaw`,
/// `parse::g711`). Everything else (G.722/MP3/AC-3/FLAC/…) falls through
/// to the Raw path and is decoded → s16le.
const AUDIO_ENCODED_OK: &[&str] = &["aac", "opus", "pcm_mulaw", "pcm_alaw"];

/// What the user asked for via `--mode`. `Auto` keeps the per-codec
/// decision; `Raw`/`Encoded` force that sender path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum RequestedMode {
    /// Encoded passthrough when every stream's codec is passthrough-
    /// eligible, otherwise Raw. The default.
    #[default]
    Auto,
    /// Force the ffmpeg-decode → SDK-re-encode path for every input.
    /// Costs CPU + one re-encode generation, but the SDK's own encoder
    /// owns the bitstream — so it answers subscriber keyframe requests
    /// (`onIntraRequestReceived`) and works for any codec ffmpeg decodes.
    Raw,
    /// Force encoded passthrough. Errors at startup if a stream's codec
    /// has no passthrough path (`decide()` returns `Err`).
    Encoded,
}

/// Per-codec natural verdict: Encoded when *both* present streams use
/// passthrough-eligible codecs; otherwise Raw. A video-only or audio-only
/// input takes only the present stream's verdict.
fn natural_mode(info: &MediaInfo) -> CodecMode {
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

/// Resolve the sender path from the probed input and the `--mode` request.
///
/// - `Auto` → the per-codec natural verdict.
/// - `Raw` → always `Raw`.
/// - `Encoded` → `Encoded`, or `Err` (naming the offending codec) when a
///   stream can't pass through — an explicit `--mode encoded` should fail
///   loudly rather than silently fall back to Raw.
pub fn decide(info: &MediaInfo, requested: RequestedMode) -> Result<CodecMode, String> {
    match requested {
        RequestedMode::Raw => Ok(CodecMode::Raw),
        RequestedMode::Auto => Ok(natural_mode(info)),
        RequestedMode::Encoded => {
            let video_bad = info.video.as_ref()
                .filter(|v| !VIDEO_ENCODED_OK.contains(&v.codec_name.as_str()))
                .map(|v| format!("video codec '{}'", v.codec_name));
            let audio_bad = info.audio.as_ref()
                .filter(|a| !AUDIO_ENCODED_OK.contains(&a.codec_name.as_str()))
                .map(|a| format!("audio codec '{}'", a.codec_name));
            let bad: Vec<String> = [video_bad, audio_bad].into_iter().flatten().collect();
            if bad.is_empty() {
                Ok(CodecMode::Encoded)
            } else {
                Err(format!(
                    "--mode encoded requested but {} has no encoded passthrough \
                     — use --mode auto or --mode raw",
                    bad.join(" and "),
                ))
            }
        }
    }
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
}

impl VideoPublisher {
    pub fn publish(&self) -> Result<(), AgoraError> {
        match self {
            VideoPublisher::Encoded(p) => p.publish(),
            VideoPublisher::Raw(p) => p.publish(),
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
    profile: Option<&str>,
) -> Result<AudioPublisher, AgoraError> {
    match mode {
        CodecMode::Encoded => {
            create_audio_encoded(svc, conn, factory, codec_name, profile)
                .map(AudioPublisher::Encoded)
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

    /// `--mode auto` shorthand for the tests below.
    fn auto(i: &MediaInfo) -> CodecMode {
        decide(i, RequestedMode::Auto).unwrap()
    }

    #[test]
    fn h264_aac_picks_encoded()  { assert_eq!(auto(&info(H264_AAC)),  CodecMode::Encoded); }
    #[test]
    fn vp9_opus_picks_encoded()  { assert_eq!(auto(&info(VP9_OPUS)),  CodecMode::Encoded); }
    #[test]
    fn mpeg2_mp3_picks_raw()     { assert_eq!(auto(&info(MPEG2_MP3)), CodecMode::Raw); }

    #[test]
    fn video_only_input_uses_video_verdict() {
        let mut i = info(H264_AAC);
        i.audio = None;
        assert_eq!(auto(&i), CodecMode::Encoded);
        let mut i = info(MPEG2_MP3);
        i.audio = None;
        assert_eq!(auto(&i), CodecMode::Raw);
    }

    #[test]
    fn audio_unsupported_forces_raw_even_if_video_ok() {
        let mut i = info(H264_AAC);
        i.audio.as_mut().unwrap().codec_name = "flac".into();
        assert_eq!(auto(&i), CodecMode::Raw);
    }

    #[test]
    fn widened_codec_combinations_pick_encoded() {
        // Every video ∈ {h264,hevc,vp8,vp9,av1} × audio ∈
        // {aac,opus,pcm_mulaw,pcm_alaw} passes the widened allowlists.
        let vids = ["h264", "hevc", "vp8", "vp9", "av1"];
        let auds = ["aac", "opus", "pcm_mulaw", "pcm_alaw"];
        for v in vids {
            for a in auds {
                let mut i = info(H264_AAC);
                i.video.as_mut().unwrap().codec_name = v.into();
                i.audio.as_mut().unwrap().codec_name = a.into();
                assert_eq!(auto(&i), CodecMode::Encoded, "{v}+{a} should be Encoded");
            }
        }
    }

    #[test]
    fn new_video_codecs_video_only_pick_encoded() {
        for v in ["hevc", "vp8", "vp9", "av1"] {
            let mut i = info(H264_AAC);
            i.video.as_mut().unwrap().codec_name = v.into();
            i.audio = None;
            assert_eq!(auto(&i), CodecMode::Encoded, "{v} video-only");
        }
    }

    #[test]
    fn still_unsupported_codecs_force_raw() {
        // A codec outside the allowlists still drags the session to Raw.
        let mut i = info(H264_AAC);
        i.video.as_mut().unwrap().codec_name = "mpeg2video".into();
        assert_eq!(auto(&i), CodecMode::Raw, "mpeg2video → Raw");
        let mut i = info(H264_AAC); // h264 ok, but g.722 audio not in list
        i.audio.as_mut().unwrap().codec_name = "g722".into();
        assert_eq!(auto(&i), CodecMode::Raw, "h264+g722 → Raw");
    }

    #[test]
    fn real_hevc_opus_fixture_probe_picks_encoded() {
        // Parsed from the actual ffprobe JSON of tests/fixtures/hevc-opus-5s.mp4.
        let i = info(HEVC_OPUS);
        assert_eq!(i.video.as_ref().unwrap().codec_name, "hevc");
        assert_eq!(i.audio.as_ref().unwrap().codec_name, "opus");
        assert_eq!(auto(&i), CodecMode::Encoded);
    }

    #[test]
    fn force_raw_overrides_a_passthrough_eligible_input() {
        // h264+aac would be Encoded under Auto; --mode raw forces Raw.
        assert_eq!(decide(&info(H264_AAC), RequestedMode::Raw), Ok(CodecMode::Raw));
        // …and a codec that's already Raw stays Raw.
        assert_eq!(decide(&info(MPEG2_MP3), RequestedMode::Raw), Ok(CodecMode::Raw));
    }

    #[test]
    fn force_encoded_ok_when_every_codec_is_eligible() {
        assert_eq!(decide(&info(H264_AAC), RequestedMode::Encoded), Ok(CodecMode::Encoded));
    }

    #[test]
    fn force_encoded_errors_and_names_the_unsupported_codec() {
        // mpeg2video + mp3 — both unsupported; the error names both.
        let err = decide(&info(MPEG2_MP3), RequestedMode::Encoded).unwrap_err();
        assert!(err.contains("mpeg2video"), "names the video codec: {err}");
        assert!(err.contains("mp3"), "names the audio codec: {err}");
        assert!(err.contains("--mode raw"), "suggests the fallback: {err}");

        // Only the audio is unsupported → only the audio is named.
        let mut i = info(H264_AAC);
        i.audio.as_mut().unwrap().codec_name = "flac".into();
        let err = decide(&i, RequestedMode::Encoded).unwrap_err();
        assert!(err.contains("flac"), "{err}");
        assert!(!err.contains("video codec"), "h264 video is fine: {err}");
    }
}
