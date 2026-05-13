//! `ffprobe -v error -of json -show_streams <input>` → MediaInfo.

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use std::path::Path;
use std::process::Command;

/// Top-level result from one probe invocation.
#[derive(Debug, Clone)]
pub struct MediaInfo {
    pub video: Option<Stream>,
    pub audio: Option<Stream>,
}

/// One stream's relevant parameters. `codec_name` matches ffprobe's lowercase
/// strings (`h264`, `aac`, `vp9`, `opus`, `mpeg2video`, `mp3`, …); the
/// `decide()` function in `agora::publisher` maps this to a CodecMode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stream {
    pub codec_name: String,
    pub profile: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub avg_frame_rate: Option<(u32, u32)>, // (num, den), e.g. (30000, 1001) for 29.97
    pub sample_rate: Option<u32>,           // audio
    pub channels: Option<u32>,              // audio
}

/// Run ffprobe and parse the JSON. `ffprobe_bin` defaults to `"ffprobe"`
/// when None (looked up on PATH).
pub fn probe(input: &Path, ffprobe_bin: Option<&Path>) -> Result<MediaInfo> {
    let bin = ffprobe_bin
        .map(|p| p.as_os_str().to_owned())
        .unwrap_or_else(|| std::ffi::OsString::from("ffprobe"));
    let out = Command::new(&bin)
        .args(["-v", "error", "-of", "json", "-show_streams"])
        .arg(input)
        .output()
        .with_context(|| format!("failed to spawn {}", bin.to_string_lossy()))?;
    if !out.status.success() {
        bail!(
            "ffprobe exited {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    parse_probe_json(&out.stdout)
}

/// Pure JSON-parsing half, separated for unit testing against canned fixtures.
pub fn parse_probe_json(bytes: &[u8]) -> Result<MediaInfo> {
    #[derive(Deserialize)]
    struct Root<'a> {
        #[serde(borrow)]
        streams: Vec<RawStream<'a>>,
    }
    #[derive(Deserialize)]
    struct RawStream<'a> {
        codec_type: &'a str,
        codec_name: Option<&'a str>,
        profile: Option<&'a str>,
        width: Option<u32>,
        height: Option<u32>,
        avg_frame_rate: Option<&'a str>,
        sample_rate: Option<serde_json::Value>, // ffprobe sometimes serializes this as a string
        channels: Option<u32>,
    }
    let root: Root = serde_json::from_slice(bytes)
        .map_err(|e| anyhow!("couldn't parse ffprobe JSON: {e}"))?;
    let mut info = MediaInfo { video: None, audio: None };
    for s in root.streams {
        let codec_name = s.codec_name.ok_or_else(|| anyhow!("stream missing codec_name"))?;
        let stream = Stream {
            codec_name: codec_name.to_string(),
            profile: s.profile.map(String::from),
            width: s.width,
            height: s.height,
            avg_frame_rate: s.avg_frame_rate.and_then(parse_fraction),
            sample_rate: s.sample_rate.and_then(|v| match v {
                serde_json::Value::Number(n) => n.as_u64().map(|x| x as u32),
                serde_json::Value::String(s) => s.parse().ok(),
                _ => None,
            }),
            channels: s.channels,
        };
        match s.codec_type {
            "video" if info.video.is_none() => info.video = Some(stream),
            "audio" if info.audio.is_none() => info.audio = Some(stream),
            "video" | "audio" => {} // ignore additional v/a streams beyond the first
            _ => {} // ignore subtitle/attachment/etc.
        }
    }
    Ok(info)
}

fn parse_fraction(s: &str) -> Option<(u32, u32)> {
    let (n, d) = s.split_once('/')?;
    let num: u32 = n.parse().ok()?;
    let den: u32 = d.parse().ok()?;
    if den == 0 { None } else { Some((num, den)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    const H264_AAC: &[u8] = include_bytes!("../../tests/fixtures/probe-h264-aac.json");
    const VP9_OPUS: &[u8] = include_bytes!("../../tests/fixtures/probe-vp9-opus.json");
    const MPEG2_MP3: &[u8] = include_bytes!("../../tests/fixtures/probe-mpeg2-mp3.json");

    #[test]
    fn h264_aac_basic_fields() {
        let info = parse_probe_json(H264_AAC).unwrap();
        let v = info.video.unwrap();
        assert_eq!(v.codec_name, "h264");
        assert_eq!(v.width, Some(320));
        assert_eq!(v.height, Some(180));
        assert_eq!(v.avg_frame_rate, Some((15, 1)));
        let a = info.audio.unwrap();
        assert_eq!(a.codec_name, "aac");
        // ffprobe spells AAC-LC's profile as "LC"
        assert_eq!(a.profile.as_deref(), Some("LC"));
        assert!(a.sample_rate.unwrap() > 0);
        assert!(a.channels.unwrap() >= 1);
    }

    #[test]
    fn vp9_opus_basic_fields() {
        let info = parse_probe_json(VP9_OPUS).unwrap();
        assert_eq!(info.video.unwrap().codec_name, "vp9");
        assert_eq!(info.audio.unwrap().codec_name, "opus");
    }

    #[test]
    fn mpeg2_mp3_basic_fields() {
        let info = parse_probe_json(MPEG2_MP3).unwrap();
        assert_eq!(info.video.unwrap().codec_name, "mpeg2video");
        assert_eq!(info.audio.unwrap().codec_name, "mp3");
    }

    #[test]
    fn parse_fraction_basic() {
        assert_eq!(parse_fraction("15/1"), Some((15, 1)));
        assert_eq!(parse_fraction("30000/1001"), Some((30000, 1001)));
        assert_eq!(parse_fraction("0/0"), None);
        assert_eq!(parse_fraction("bad"), None);
    }

    #[test]
    fn malformed_json_errors_clearly() {
        let err = parse_probe_json(b"{not json").unwrap_err();
        assert!(err.to_string().contains("ffprobe JSON"));
    }
}
