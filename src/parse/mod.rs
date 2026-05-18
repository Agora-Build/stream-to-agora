//! Frame parsers that read off ffmpeg's pipes. One per (codec × mode):
//!   - aac:  ADTS-framed AAC (from `ffmpeg -f adts`)
//!   - opus: Ogg-encapsulated Opus (from `ffmpeg -f ogg`)
//!   - h264: Annex-B NALU stream (from `ffmpeg -f h264`)
//!   - hevc: Annex-B H.265 NALU stream (from `ffmpeg -f hevc`)
//!   - yuv:  planar yuv420p (from `ffmpeg -pix_fmt yuv420p -f rawvideo`)
//!   - pcm:  interleaved s16le (from `ffmpeg -f s16le`)

pub mod aac;
pub mod h264;
pub mod hevc;
pub mod opus;
pub mod yuv;
pub mod pcm;
