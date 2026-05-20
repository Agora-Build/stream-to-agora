//! Frame-size helper for interleaved s16le PCM. The raw-audio pump reads
//! exactly this many bytes from ffmpeg's `-f s16le` stdout per chunk
//! (default 10 ms = `sample_rate / 100` samples per channel); no
//! in-band framing.

/// Required byte count for one chunk: `samples_per_channel * channels * 2`.
pub fn frame_bytes(samples_per_channel: u32, channels: u32) -> usize {
    samples_per_channel as usize * channels as usize * 2
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ten_ms_48k_stereo_is_1920_bytes() {
        assert_eq!(frame_bytes(480, 2), 1920);
    }
}
