//! ADTS-framed AAC: walk a byte slice, yield one frame per call.
//!
//! ADTS header (7 bytes, 9 with CRC):
//!   syncword 12 bits       = 0xFFF
//!   id        1            — MPEG version
//!   layer     2            = 00
//!   protection 1           (0 = CRC present, +2 bytes after header)
//!   profile   2            — 1 = AAC LC
//!   sr_index  4            — index into the 13-entry sample-rate table
//!   private   1
//!   channel_config 3
//!   ... (5 more bits we don't need)
//!   frame_length 13        — total ADTS frame incl. header
//!   buffer_fullness 11
//!   raw_blocks 2           — N-1 raw data blocks; 0 means 1 block per frame

const SAMPLE_RATES: [u32; 13] = [
    96000, 88200, 64000, 48000, 44100, 32000,
    24000, 22050, 16000, 12000, 11025, 8000, 7350,
];

/// One AAC frame extracted from an ADTS-framed byte stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AacFrame<'a> {
    pub data: &'a [u8],           // includes ADTS header bytes
    pub sample_rate: u32,
    pub channels: u32,
    pub samples_per_channel: u32, // 1024 per frame for AAC LC
}

/// Try to read one AAC frame starting at byte 0 of `buf`. Returns
/// `Ok(Some(frame))` when a complete frame is available, `Ok(None)` if
/// `buf` is shorter than the ADTS header (caller should read more bytes),
/// or `Err` if the bytes at offset 0 don't look like ADTS.
pub fn next_frame(buf: &[u8]) -> Result<Option<AacFrame<'_>>, &'static str> {
    if buf.len() < 7 { return Ok(None); }
    // Sync word: 12 ones (0xFFF)
    if buf[0] != 0xFF || (buf[1] & 0xF0) != 0xF0 {
        return Err("ADTS sync word missing");
    }
    let has_crc = (buf[1] & 0x01) == 0;
    let header_len = if has_crc { 9 } else { 7 };
    let profile = ((buf[2] >> 6) & 0x03) + 1; // 1=LC, 2=Main, ...
    let sr_index = ((buf[2] >> 2) & 0x0F) as usize;
    if sr_index >= SAMPLE_RATES.len() { return Err("ADTS bad sample-rate index"); }
    let channels = (((buf[2] & 0x01) << 2) | (buf[3] >> 6)) as u32;
    if channels == 0 { return Err("ADTS channel config 0 (PCE) not supported"); }
    let frame_length: u32 =
        ((buf[3] as u32 & 0x03) << 11) |
        ((buf[4] as u32) << 3) |
        ((buf[5] as u32 & 0xE0) >> 5);
    if (frame_length as usize) < header_len { return Err("ADTS frame_length < header"); }
    if buf.len() < frame_length as usize { return Ok(None); }
    let _ = profile; // unused; we treat all profiles as AAC-LC for samples-per-channel
    Ok(Some(AacFrame {
        data: &buf[..frame_length as usize],
        sample_rate: SAMPLE_RATES[sr_index],
        channels,
        samples_per_channel: 1024,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Synthesize an ADTS frame: AAC-LC, given sr_index + channels + length.
    fn synth(frame_length: u16, sr_index: u8, channels: u8) -> Vec<u8> {
        let profile = 1; // LC
        let mut v = vec![
            0xFF,                                                        // syncword high
            0xF1,                                                        // syncword low + version=0 + layer=00 + no CRC
            ((profile << 6) | (sr_index << 2) | ((channels >> 2) & 0x1)) as u8,
            (((channels & 0x03) << 6) | ((frame_length >> 11) as u8 & 0x03)) as u8,
            ((frame_length >> 3) as u8),
            (((frame_length << 5) as u8) & 0xE0) | 0x1F,
            0xFC,                                                        // buffer fullness + raw_blocks=0
        ];
        v.resize(frame_length as usize, 0xAA);
        v
    }

    #[test]
    fn parses_one_aac_lc_44k_stereo_frame() {
        let f = synth(64, 4, 2);
        let frame = next_frame(&f).unwrap().unwrap();
        assert_eq!(frame.data.len(), 64);
        assert_eq!(frame.sample_rate, 44100);
        assert_eq!(frame.channels, 2);
        assert_eq!(frame.samples_per_channel, 1024);
    }

    #[test]
    fn returns_none_when_buf_too_short() {
        let f = synth(64, 4, 2);
        assert!(next_frame(&f[..3]).unwrap().is_none());   // shorter than header
        assert!(next_frame(&f[..40]).unwrap().is_none());  // header parsed, body short
    }

    #[test]
    fn rejects_missing_syncword() {
        let bytes = b"\x00\x00\x00\x00\x00\x00\x00";
        assert!(next_frame(bytes).is_err());
    }

    #[test]
    fn rejects_bad_sr_index() {
        let mut f = synth(64, 4, 2);
        f[2] = (f[2] & !0x3C) | (15 << 2); // sr_index = 15 (reserved)
        assert!(next_frame(&f).is_err());
    }

    #[test]
    fn samples_per_channel_constant() {
        let f = synth(120, 3, 1); // 48 kHz, mono
        let frame = next_frame(&f).unwrap().unwrap();
        assert_eq!(frame.sample_rate, 48000);
        assert_eq!(frame.channels, 1);
        assert_eq!(frame.samples_per_channel, 1024);
    }
}
