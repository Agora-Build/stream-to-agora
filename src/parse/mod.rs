//! Frame parsers that read off ffmpeg's pipes. One per (codec × mode):
//!   - aac: ADTS-framed AAC (from `ffmpeg -f adts`)
//!   - h264: Annex-B NALU stream (from `ffmpeg -f h264`)  [P2-T5]
//!   - yuv:  planar yuv420p (from `ffmpeg -pix_fmt yuv420p -f rawvideo`)  [P2-T6]
//!   - pcm:  interleaved s16le (from `ffmpeg -f s16le`)  [P2-T6]

pub mod aac;
// h264 / yuv / pcm modules added in P2-T5 / P2-T6.
