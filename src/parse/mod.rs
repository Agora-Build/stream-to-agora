//! Frame parsers that read off ffmpeg's pipes. One per (codec × mode):
//!   - aac:  ADTS-framed AAC / HE-AAC (from `ffmpeg -f adts`)
//!   - opus: Ogg-encapsulated Opus (from `ffmpeg -f ogg`)
//!   - g711: raw µ-law/A-law (from `ffmpeg -f mulaw` / `-f alaw`)
//!   - h264: Annex-B NALU stream (from `ffmpeg -f h264`)
//!   - hevc: Annex-B H.265 NALU stream (from `ffmpeg -f hevc`)
//!   - ivf:  IVF-framed VP8 / VP9 / AV1 (from `ffmpeg -f ivf`)
//!   - yuv:  planar yuv420p (from `ffmpeg -pix_fmt yuv420p -f rawvideo`)
//!   - pcm:  interleaved s16le (from `ffmpeg -f s16le`)

pub mod aac;
pub mod g711;
pub mod h264;
pub mod hevc;
pub mod ivf;
pub mod opus;
pub mod yuv;
pub mod pcm;
