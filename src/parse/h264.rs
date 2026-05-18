//! Annex-B NALU walker for H.264. Yields one access-unit per call.
//!
//! An access unit = the first NAL start code through the last NAL of one
//! coded picture. The AU ends right before the next NAL that begins a new
//! AU: a non-VCL NAL of type SEI(6)/SPS(7)/PPS(8)/AUD(9), or a VCL slice
//! (type 1-5) whose `first_mb_in_slice == 0` (first slice of a new
//! picture). This groups SPS+PPS+IDR into the keyframe AU rather than
//! splitting blindly at the second VCL slice — which stranded the IDR
//! without its parameter sets and glued them onto the preceding delta
//! frame, leaving the subscriber unable to decode (endless intra-frame
//! requests / black video). Mirrors the SDK sample's
//! `HelperH264FileParser::getH264Frame` byte-for-byte.
//!
//! VCL NALU types: 1 (non-IDR slice), 5 (IDR slice). Non-VCL types
//! include 6 (SEI), 7 (SPS), 8 (PPS), 9 (AUD), etc.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct H264Au<'a> {
    pub data: &'a [u8],
    /// True only when the AU is a self-contained sync point: an IDR slice
    /// *together with* the SPS and PPS needed to decode it (matches the
    /// SDK sample's `is_key_frame && is_pps && is_sps`).
    pub is_keyframe: bool,
}

/// Try to read one access-unit. Returns `Ok(Some(au))` when a complete
/// AU is available (i.e. the *next* VCL NALU's start code has been seen,
/// or `eof` is true), `Ok(None)` if more data is needed.
///
/// `eof = true` signals that no more bytes are coming, so emit what's left.
pub fn next_au(buf: &[u8], eof: bool) -> Result<Option<H264Au<'_>>, &'static str> {
    let starts = find_start_codes(buf);
    if starts.is_empty() {
        return if eof && !buf.is_empty() {
            Err("trailing bytes without H.264 start code")
        } else {
            Ok(None)
        };
    }
    let start = starts[0]; // include any prefixed SPS/PPS/SEI from the very front
    let mut seen_vcl = false;
    let mut is_sps = false;
    let mut is_pps = false;
    let mut seen_idr = false;

    for &p in &starts {
        let t = nal_unit_type_at(buf, p);
        if !seen_vcl {
            if t == 7 {
                is_sps = true;
            } else if t == 8 {
                is_pps = true;
            }
            if is_vcl(t) {
                seen_vcl = true;
                if t == 5 {
                    seen_idr = true;
                }
            }
            continue;
        }
        // First VCL slice already seen: this NAL either continues the
        // current picture (a slice with first_mb_in_slice > 0) or starts
        // a new access unit.
        if is_au_delimiter_nal(t) {
            return Ok(Some(H264Au {
                data: &buf[start..p],
                is_keyframe: seen_idr && is_sps && is_pps,
            }));
        }
        if is_vcl(t) {
            match slice_first_mb_at(buf, p) {
                None => {
                    // Slice header not fully buffered yet.
                    if eof {
                        break;
                    }
                    return Ok(None);
                }
                Some(0) => {
                    return Ok(Some(H264Au {
                        data: &buf[start..p],
                        is_keyframe: seen_idr && is_sps && is_pps,
                    }));
                }
                Some(_) => {
                    if t == 5 {
                        seen_idr = true; // continuation slice of an IDR picture
                    }
                }
            }
        }
    }

    if !seen_vcl {
        return Ok(None); // no picture yet; wait for more
    }
    if !eof {
        return Ok(None); // need more bytes to find the AU boundary
    }
    Ok(Some(H264Au {
        data: &buf[start..],
        is_keyframe: seen_idr && is_sps && is_pps,
    }))
}

fn is_vcl(t: u8) -> bool {
    (1..=5).contains(&t)
}

/// NAL types that may only appear *before* the first slice of a picture;
/// encountering one after a slice means a new access unit has started.
fn is_au_delimiter_nal(t: u8) -> bool {
    matches!(t, 6 | 7 | 8 | 9) // SEI / SPS / PPS / AUD
}

/// Read `first_mb_in_slice` — the leading `ue(v)` of a slice header — from
/// the VCL NAL whose start code begins at `p`. De-emulates the first few
/// RBSP bytes. Returns `None` if the header isn't fully buffered yet.
fn slice_first_mb_at(buf: &[u8], p: usize) -> Option<u32> {
    let sc_len = if buf.get(p..p + 4) == Some(&[0, 0, 0, 1]) { 4 } else { 3 };
    let mut rbsp = [0u8; 8];
    let mut got = 0;
    let mut zeros = 0;
    let mut q = p + sc_len + 1; // skip start code + 1-byte NAL header
    while q < buf.len() && got < rbsp.len() {
        let c = buf[q];
        if zeros >= 2 && c == 0x03 {
            zeros = 0;
            q += 1;
            continue; // emulation-prevention byte
        }
        rbsp[got] = c;
        got += 1;
        zeros = if c == 0 { zeros + 1 } else { 0 };
        q += 1;
    }
    if got == 0 {
        return None;
    }
    let total_bits = got * 8;
    let mut bit = 0usize;
    let mut lz = 0usize;
    while bit < total_bits && (rbsp[bit >> 3] >> (7 - (bit & 7))) & 1 == 0 {
        lz += 1;
        bit += 1;
    }
    if bit >= total_bits {
        return None; // ue(v) not complete in the buffered window
    }
    bit += 1; // the stop 1-bit
    if bit + lz > total_bits {
        return None;
    }
    let mut val: u32 = 0;
    for _ in 0..lz {
        val = (val << 1) | ((rbsp[bit >> 3] >> (7 - (bit & 7))) & 1) as u32;
        bit += 1;
    }
    Some((1u32 << lz) - 1 + val)
}

/// Byte offsets in `buf` where a 3- or 4-byte start code BEGINS.
fn find_start_codes(buf: &[u8]) -> Vec<usize> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 3 <= buf.len() {
        if buf[i] == 0 && buf[i+1] == 0 {
            if i + 4 <= buf.len() && buf[i+2] == 0 && buf[i+3] == 1 {
                out.push(i); i += 4; continue;
            }
            if buf[i+2] == 1 { out.push(i); i += 3; continue; }
        }
        i += 1;
    }
    out
}

fn nal_unit_type_at(buf: &[u8], start: usize) -> u8 {
    let header_off = if buf.get(start..start+4) == Some(&[0,0,0,1]) { start + 4 } else { start + 3 };
    buf.get(header_off).map(|b| b & 0x1F).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sc4() -> Vec<u8> { vec![0, 0, 0, 1] }
    /// Build a single NALU: start code + header byte + `first_mb`(=0) ue(v)
    /// bit + padding so a slice header is parseable. The leading payload
    /// byte `0x80` encodes `first_mb_in_slice = 0` (single leading 1-bit).
    fn vcl(nal_type: u8, len: usize) -> Vec<u8> {
        let mut v = sc4();
        v.push(nal_type & 0x1F);
        v.push(0x80); // first_mb_in_slice = ue(0)
        v.extend(std::iter::repeat(0xAB).take(len.saturating_sub(2)));
        v
    }
    /// Non-VCL NALU (SPS/PPS/SEI/AUD): start code + header byte + payload.
    fn nonvcl(nal_type: u8, len: usize) -> Vec<u8> {
        let mut v = sc4();
        v.push(nal_type & 0x1F);
        v.extend(std::iter::repeat(0xAB).take(len.saturating_sub(1)));
        v
    }

    #[test]
    fn bare_idr_is_not_a_keyframe_without_param_sets() {
        // Sample-faithful: is_key requires IDR *and* SPS *and* PPS.
        let buf = vcl(5, 100);
        let au = next_au(&buf, true).unwrap().unwrap();
        assert_eq!(au.data.len(), buf.len());
        assert!(!au.is_keyframe);
    }

    #[test]
    fn sps_pps_idr_collapsed_into_one_keyframe_au() {
        let mut buf = nonvcl(7, 5);
        buf.extend(nonvcl(8, 5));
        buf.extend(vcl(5, 50));
        let au = next_au(&buf, true).unwrap().unwrap();
        assert_eq!(au.data.len(), buf.len());
        assert!(au.is_keyframe);
    }

    #[test]
    fn keyframe_param_sets_not_glued_onto_preceding_delta() {
        // The bug: [P][SPS][PPS][IDR][P] used to split as
        // [P][SPS][PPS] + [IDR][P]. Correct: [P] then [SPS][PPS][IDR].
        let mut buf = vcl(1, 20); // P frame
        let p_len = buf.len();
        buf.extend(nonvcl(7, 5)); // SPS  ─┐
        buf.extend(nonvcl(8, 5)); // PPS   ├ next keyframe AU
        buf.extend(vcl(5, 40)); //   IDR ─┘
        buf.extend(vcl(1, 10)); // following P (boundary marker)

        let au = next_au(&buf, false).unwrap().unwrap();
        assert_eq!(au.data.len(), p_len, "first AU must be the lone P frame");
        assert!(!au.is_keyframe);

        let rest = &buf[p_len..];
        let au2 = next_au(rest, false).unwrap().unwrap();
        // AU2 = SPS+PPS+IDR, ends right before the trailing P.
        assert_eq!(au2.data.len(), rest.len() - (sc4().len() + 2 + 8));
        assert!(au2.is_keyframe, "IDR must carry its SPS/PPS and be a keyframe");
    }

    #[test]
    fn consecutive_delta_frames_split_each_picture() {
        let mut buf = vcl(1, 30);
        let first = buf.len();
        buf.extend(vcl(1, 20));
        let au = next_au(&buf, false).unwrap().unwrap();
        assert_eq!(au.data.len(), first);
        assert!(!au.is_keyframe);
    }

    #[test]
    fn returns_none_when_need_more() {
        let buf = vcl(1, 30);
        assert!(next_au(&buf, false).unwrap().is_none());
    }

    #[test]
    fn finds_3byte_and_4byte_start_codes() {
        // 3-byte start code IDR, then 4-byte start code P boundary.
        let mut buf = vec![0, 0, 1, 0x65, 0x80];
        buf.extend([0xAB; 20]);
        buf.extend([0, 0, 0, 1, 0x41, 0x80]);
        buf.extend([0xAB; 20]);
        let au = next_au(&buf, false).unwrap().unwrap();
        assert!(!au.is_keyframe); // bare IDR, no SPS/PPS
        assert_eq!(au.data.len(), 3 + 2 + 20);
    }

    #[test]
    fn empty_returns_none() {
        assert!(next_au(&[], true).unwrap().is_none());
    }
}
