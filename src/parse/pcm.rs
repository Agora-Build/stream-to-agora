//! Fixed-size s16le interleaved PCM reader. One "frame" is configurable;
//! Phase 2 defaults to 10 ms = (sample_rate / 100) samples per channel.

/// Required byte count for one frame.
pub fn frame_bytes(samples_per_channel: u32, channels: u32) -> usize {
    samples_per_channel as usize * channels as usize * 2
}

pub fn next_frame(buf: &[u8], samples_per_channel: u32, channels: u32)
    -> Result<Option<&[u8]>, &'static str>
{
    if samples_per_channel == 0 || channels == 0 { return Err("zero size"); }
    let need = frame_bytes(samples_per_channel, channels);
    if buf.len() < need { Ok(None) } else { Ok(Some(&buf[..need])) }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn ten_ms_48k_stereo_is_1920_bytes() {
        assert_eq!(frame_bytes(480, 2), 1920);
    }
    #[test] fn returns_frame_when_full() {
        let buf = vec![0u8; 1920 * 3];
        let f = next_frame(&buf, 480, 2).unwrap().unwrap();
        assert_eq!(f.len(), 1920);
    }
    #[test] fn returns_none_when_short() {
        let buf = vec![0u8; 100];
        assert!(next_frame(&buf, 480, 2).unwrap().is_none());
    }
}
