//! ffmpeg / ffprobe glue: probe an input file to learn its codec ids and
//! stream parameters, then run ffmpeg as a child to emit demuxed-or-decoded
//! frames on stdout/extra-fd pipes.

pub mod probe;

pub use probe::{probe, MediaInfo, Stream};
