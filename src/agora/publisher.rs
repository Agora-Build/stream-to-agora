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

/// The video codec names ffprobe reports for codecs Agora's encoded
/// `agora_video_encoded_image_sender_send` accepts.
const VIDEO_ENCODED_OK: &[&str] = &["h264", "hevc", "vp8", "vp9", "av1", "mjpeg"];

/// Audio codec names for `agora_audio_encoded_frame_sender_send`. ffprobe
/// reports `aac` for all AAC profiles; the encoded-frame info struct's
/// `codec` field is set per-profile in `agora::audio` later.
const AUDIO_ENCODED_OK: &[&str] = &["aac", "opus", "pcm_alaw", "pcm_mulaw", "g722"];

/// Pure mode-decision. Encoded if *both* present streams use accepted
/// codecs; otherwise Raw. A video-only or audio-only input takes only
/// the present stream's verdict.
pub fn decide(info: &MediaInfo) -> CodecMode {
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

/// Placeholder — replaced wholesale in Task 11 with `Encoded(EncodedAudioPublisher) / Raw(RawAudioPublisher)`.
pub enum AudioPublisher { Placeholder }
/// Placeholder — replaced wholesale in Task 11.
pub enum VideoPublisher { Placeholder }

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ffmpeg::probe::parse_probe_json;

    const H264_AAC: &[u8] = include_bytes!("../../tests/fixtures/probe-h264-aac.json");
    const VP9_OPUS: &[u8] = include_bytes!("../../tests/fixtures/probe-vp9-opus.json");
    const MPEG2_MP3: &[u8] = include_bytes!("../../tests/fixtures/probe-mpeg2-mp3.json");

    fn info(json: &[u8]) -> MediaInfo {
        parse_probe_json(json).unwrap()
    }

    #[test]
    fn h264_aac_picks_encoded()  { assert_eq!(decide(&info(H264_AAC)),  CodecMode::Encoded); }
    #[test]
    fn vp9_opus_picks_encoded()  { assert_eq!(decide(&info(VP9_OPUS)),  CodecMode::Encoded); }
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
}
