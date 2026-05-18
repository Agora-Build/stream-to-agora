//! G.711 (µ-law / A-law) framer. ffmpeg `-c:a copy -f mulaw`/`-f alaw`
//! emits a headerless stream of 8-bit companded samples (one byte per
//! sample per channel). There is no in-band framing, so we slice it
//! into fixed real-time chunks the SDK's encoded audio sender expects.
//!
//! chunk bytes = sample_rate/1000 * frame_ms * channels
//! (20 ms @ 8 kHz mono = 160 bytes = 160 samples/channel).

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct G711Frame<'a> {
    pub data: &'a [u8],
    pub sample_rate: u32,
    pub channels: u32,
    pub samples_per_channel: u32,
}

/// Slice one `frame_ms` chunk off the front of `buf`. `None` until a
/// whole chunk is buffered; the caller drains `data.len()`.
pub fn next_frame(
    buf: &[u8],
    sample_rate: u32,
    channels: u32,
    frame_ms: u32,
) -> Option<G711Frame<'_>> {
    let sr = sample_rate.max(1);
    let ch = channels.max(1);
    let fm = frame_ms.max(1);
    let samples_per_channel = sr / 1000 * fm;
    let need = (samples_per_channel * ch) as usize;
    if need == 0 || buf.len() < need {
        return None;
    }
    Some(G711Frame {
        data: &buf[..need],
        sample_rate: sr,
        channels: ch,
        samples_per_channel,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_20ms_8k_mono() {
        let buf = vec![0xD5u8; 160];
        let f = next_frame(&buf, 8000, 1, 20).unwrap();
        assert_eq!(f.data.len(), 160);
        assert_eq!(f.samples_per_channel, 160);
        assert_eq!(f.sample_rate, 8000);
        assert_eq!(f.channels, 1);
    }

    #[test]
    fn need_more_when_short() {
        let buf = vec![0u8; 159];
        assert!(next_frame(&buf, 8000, 1, 20).is_none());
    }

    #[test]
    fn stereo_doubles_chunk() {
        let buf = vec![0u8; 320];
        let f = next_frame(&buf, 8000, 2, 20).unwrap();
        assert_eq!(f.data.len(), 320); // 160 samples/ch * 2 ch
        assert_eq!(f.samples_per_channel, 160);
        assert_eq!(f.channels, 2);
    }

    #[test]
    fn ten_ms_16k() {
        let buf = vec![0u8; 160];
        let f = next_frame(&buf, 16000, 1, 10).unwrap();
        assert_eq!(f.samples_per_channel, 160); // 16000/1000*10
        assert_eq!(f.data.len(), 160);
    }

    #[test]
    fn leftover_after_one_frame_is_caller_drained() {
        let buf = vec![7u8; 200];
        let f = next_frame(&buf, 8000, 1, 20).unwrap();
        assert_eq!(f.data.len(), 160); // caller drains 160, 40 remain
    }
}
