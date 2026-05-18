//! IVF demuxer shared by VP8 / VP9 / AV1. ffmpeg `-c:v copy -f ivf`
//! wraps the copied stream in IVF: a 32-byte file header
//! (`"DKIF"`, version, header_len, FourCC, w, h, rate, scale,
//! frame_count, reserved) followed by, per frame, a 12-byte header
//! (`frame_size:u32 LE`, `timestamp:u64 LE`) and the payload.
//!
//! The container split is codec-agnostic; only the keyframe test
//! differs per codec:
//!   - VP8 : bit 0 of the first payload byte (`key_frame`, 0 = key).
//!   - VP9 : the uncompressed header (`frame_marker`, profile,
//!           `show_existing_frame`, `frame_type`).
//!   - AV1 : the temporal unit carries an `OBU_SEQUENCE_HEADER` —
//!           encoders emit one at every random-access (key) point.
//!
//! Like `OpusDemux`, `IvfReader` owns draining the caller's buffer
//! (IVF framing ≠ payload length) and yields owned frames.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IvfCodec {
    Vp8,
    Vp9,
    Av1,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IvfFrame {
    pub data: Vec<u8>,
    pub is_keyframe: bool,
}

pub struct IvfReader {
    codec: IvfCodec,
    header_seen: bool,
}

impl IvfReader {
    pub fn new(codec: IvfCodec) -> Self {
        IvfReader {
            codec,
            header_seen: false,
        }
    }

    /// Pull the next coded frame, consuming the IVF file header (once)
    /// and the 12-byte frame header from the front of `buf`.
    /// `Ok(None)` if more bytes are needed; `Err` on a bad file header.
    pub fn pull(&mut self, buf: &mut Vec<u8>) -> Result<Option<IvfFrame>, &'static str> {
        if !self.header_seen {
            if buf.len() < 32 {
                return Ok(None);
            }
            if &buf[0..4] != b"DKIF" {
                return Err("IVF: bad DKIF magic");
            }
            let hdr_len = u16::from_le_bytes([buf[6], buf[7]]) as usize;
            let hdr_len = hdr_len.max(32); // spec: 32; be defensive
            if buf.len() < hdr_len {
                return Ok(None);
            }
            buf.drain(0..hdr_len);
            self.header_seen = true;
        }
        if buf.len() < 12 {
            return Ok(None);
        }
        let frame_size = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
        let total = 12 + frame_size;
        if buf.len() < total {
            return Ok(None);
        }
        let payload: Vec<u8> = buf[12..total].to_vec();
        buf.drain(0..total);
        let is_keyframe = match self.codec {
            IvfCodec::Vp8 => vp8_is_keyframe(&payload),
            IvfCodec::Vp9 => vp9_is_keyframe(&payload),
            IvfCodec::Av1 => av1_is_keyframe(&payload),
        };
        Ok(Some(IvfFrame {
            data: payload,
            is_keyframe,
        }))
    }
}

/// VP8 frame tag: the low bit of byte 0 is `key_frame` (0 = key frame,
/// 1 = interframe) — RFC 6386 §9.1.
fn vp8_is_keyframe(p: &[u8]) -> bool {
    !p.is_empty() && (p[0] & 0x01) == 0
}

/// VP9 uncompressed header (VP9 bitstream spec §6.2), MSB-first.
fn vp9_is_keyframe(p: &[u8]) -> bool {
    let mut r = BitReader::new(p);
    // frame_marker f(2) == 2
    if r.f(2) != Some(2) {
        return false;
    }
    let profile_low = r.f(1);
    let profile_high = r.f(1);
    let (Some(lo), Some(hi)) = (profile_low, profile_high) else {
        return false;
    };
    let profile = (hi << 1) | lo;
    if profile == 3 && r.f(1).is_none() {
        return false; // reserved_zero
    }
    match r.f(1) {
        Some(1) => false,           // show_existing_frame → not a new (key) frame
        Some(0) => r.f(1) == Some(0), // frame_type: 0 = KEY_FRAME
        _ => false,
    }
}

/// AV1: a temporal unit is a key/random-access point iff it carries an
/// OBU_SEQUENCE_HEADER (type 1) — libaom/SVT-AV1 emit one at every key
/// frame for random access. Walk OBU headers + LEB128 sizes (AV1 spec
/// §5.3).
fn av1_is_keyframe(p: &[u8]) -> bool {
    let mut i = 0usize;
    while i < p.len() {
        let hdr = p[i];
        let obu_type = (hdr >> 3) & 0x0F;
        let ext = (hdr >> 2) & 0x01;
        let has_size = (hdr >> 1) & 0x01;
        i += 1;
        if ext == 1 {
            i += 1; // obu_extension_header (temporal/spatial id)
        }
        if i > p.len() {
            return false;
        }
        if obu_type == 1 {
            return true; // OBU_SEQUENCE_HEADER
        }
        let payload_len = if has_size == 1 {
            let (sz, n) = match leb128(&p[i..]) {
                Some(v) => v,
                None => return false,
            };
            i += n;
            sz
        } else {
            p.len().saturating_sub(i) // last OBU runs to end
        };
        i += payload_len;
    }
    false
}

/// Unsigned LEB128 (AV1 spec §4.10.5): up to 8 bytes, 7 bits each,
/// LSB-first, continuation in the high bit. Returns (value, bytes).
fn leb128(b: &[u8]) -> Option<(usize, usize)> {
    let mut value: usize = 0;
    for i in 0..8 {
        let byte = *b.get(i)?;
        value |= ((byte & 0x7F) as usize) << (i * 7);
        if byte & 0x80 == 0 {
            return Some((value, i + 1));
        }
    }
    None
}

/// Minimal MSB-first bit reader for the VP9 uncompressed header.
struct BitReader<'a> {
    b: &'a [u8],
    bit: usize,
}
impl<'a> BitReader<'a> {
    fn new(b: &'a [u8]) -> Self {
        BitReader { b, bit: 0 }
    }
    /// Read `n` bits (n ≤ 32) MSB-first; `None` if past the end.
    fn f(&mut self, n: usize) -> Option<u32> {
        let mut v = 0u32;
        for _ in 0..n {
            let byte = self.b.get(self.bit >> 3)?;
            let shift = 7 - (self.bit & 7);
            v = (v << 1) | ((byte >> shift) & 1) as u32;
            self.bit += 1;
        }
        Some(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ivf_header(fourcc: &[u8; 4]) -> Vec<u8> {
        let mut h = b"DKIF".to_vec();
        h.extend([0, 0]); // version
        h.extend([32, 0]); // header length = 32
        h.extend(fourcc);
        h.extend([0u8; 32 - 12]); // w,h,rate,scale,count,reserved
        h
    }
    fn ivf_frame(payload: &[u8]) -> Vec<u8> {
        let mut f = (payload.len() as u32).to_le_bytes().to_vec();
        f.extend(0u64.to_le_bytes()); // timestamp
        f.extend(payload);
        f
    }

    #[test]
    fn vp8_keyframe_then_delta() {
        let mut buf = ivf_header(b"VP80");
        buf.extend(ivf_frame(&[0x00, 0xAA, 0xBB])); // bit0=0 → key
        buf.extend(ivf_frame(&[0x01, 0xCC])); // bit0=1 → delta
        let mut r = IvfReader::new(IvfCodec::Vp8);
        let a = r.pull(&mut buf).unwrap().unwrap();
        assert!(a.is_keyframe);
        assert_eq!(a.data, vec![0x00, 0xAA, 0xBB]);
        let b = r.pull(&mut buf).unwrap().unwrap();
        assert!(!b.is_keyframe);
        assert_eq!(b.data, vec![0x01, 0xCC]);
        assert!(r.pull(&mut buf).unwrap().is_none());
    }

    #[test]
    fn vp9_keyframe_then_delta() {
        // profile 0, show_existing_frame=0; KEY: 0b100000.. = 0x80,
        // NON-KEY: frame_type bit set → 0x84.
        let mut buf = ivf_header(b"VP90");
        buf.extend(ivf_frame(&[0x80, 0x11])); // key
        buf.extend(ivf_frame(&[0x84, 0x22])); // delta
        let mut r = IvfReader::new(IvfCodec::Vp9);
        assert!(r.pull(&mut buf).unwrap().unwrap().is_keyframe);
        assert!(!r.pull(&mut buf).unwrap().unwrap().is_keyframe);
    }

    #[test]
    fn av1_keyframe_needs_sequence_header() {
        // TD(0x12 sz0) + SEQ(0x0A sz1 [0x00]) + FRAME(0x32 sz1 [0x00]) → key
        let key_tu = [0x12, 0x00, 0x0A, 0x01, 0x00, 0x32, 0x01, 0x00];
        // TD + FRAME only → no seq header → delta
        let delta_tu = [0x12, 0x00, 0x32, 0x01, 0x00];
        let mut buf = ivf_header(b"AV01");
        buf.extend(ivf_frame(&key_tu));
        buf.extend(ivf_frame(&delta_tu));
        let mut r = IvfReader::new(IvfCodec::Av1);
        assert!(r.pull(&mut buf).unwrap().unwrap().is_keyframe);
        assert!(!r.pull(&mut buf).unwrap().unwrap().is_keyframe);
    }

    #[test]
    fn need_more_until_full_frame() {
        let mut buf = ivf_header(b"VP80");
        buf.extend(ivf_frame(&[0x00; 50]));
        buf.truncate(32 + 12 + 20); // header + partial frame
        let mut r = IvfReader::new(IvfCodec::Vp8);
        assert!(r.pull(&mut buf).unwrap().is_none());
    }

    #[test]
    fn partial_file_header_need_more() {
        let mut buf = b"DKIF\x00\x00".to_vec();
        let mut r = IvfReader::new(IvfCodec::Vp9);
        assert!(r.pull(&mut buf).unwrap().is_none());
    }

    #[test]
    fn bad_magic_errs() {
        let mut buf = vec![0u8; 40];
        let mut r = IvfReader::new(IvfCodec::Vp8);
        assert!(r.pull(&mut buf).is_err());
    }

    #[test]
    fn leb128_multibyte() {
        assert_eq!(leb128(&[0x00]), Some((0, 1)));
        assert_eq!(leb128(&[0x7F]), Some((127, 1)));
        assert_eq!(leb128(&[0x80, 0x01]), Some((128, 2)));
        assert_eq!(leb128(&[0xC1, 0x02]), Some((321, 2)));
    }

    /// Local cross-check vs ffprobe on a real ffmpeg `-f ivf` stream.
    /// Inert unless `STA_REAL_IVF` (path), `STA_REAL_IVF_CODEC`
    /// (vp8|vp9|av1) and `STA_REAL_IVF_OUT` are set. Writes
    /// "<idx> <payload_len> <is_key>" per frame.
    #[test]
    fn crosscheck_real_ivf() {
        let (Ok(inp), Ok(codec), Ok(outp)) = (
            std::env::var("STA_REAL_IVF"),
            std::env::var("STA_REAL_IVF_CODEC"),
            std::env::var("STA_REAL_IVF_OUT"),
        ) else {
            return;
        };
        let codec = match codec.as_str() {
            "vp8" => IvfCodec::Vp8,
            "vp9" => IvfCodec::Vp9,
            "av1" => IvfCodec::Av1,
            _ => panic!("bad STA_REAL_IVF_CODEC"),
        };
        let mut buf = std::fs::read(&inp).expect("read STA_REAL_IVF");
        let mut r = IvfReader::new(codec);
        let mut out = String::new();
        let mut idx = 0usize;
        while let Some(f) = r.pull(&mut buf).expect("ivf parse") {
            out.push_str(&format!(
                "{} {} {}\n",
                idx,
                f.data.len(),
                if f.is_keyframe { 1 } else { 0 }
            ));
            idx += 1;
        }
        std::fs::write(&outp, out).expect("write STA_REAL_IVF_OUT");
        eprintln!("rust ivf frames={idx}");
    }
}
