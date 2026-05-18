//! Annex-B NALU walker for H.265 / HEVC. Yields one access-unit per call.
//!
//! Structurally identical to `parse::h264::next_au` (same fixed
//! group-the-picture logic that resolved the H.264 black-video bug) but
//! with HEVC NAL semantics:
//!
//! - NAL header is **2 bytes**; `nal_type = (byte0 >> 1) & 0x3F`.
//! - VCL NAL types are `0..=31`. IRAP (random-access / "keyframe")
//!   pictures are `16..=23` (BLA 16-18, IDR 19-20, CRA 21, rsv 22-23).
//! - Parameter / delimiter NALs that may only appear before the first
//!   slice of a picture: VPS(32), SPS(33), PPS(34), AUD(35),
//!   prefix-SEI(39). Seeing one after a slice means a new AU started.
//! - A picture's first slice has `first_slice_segment_in_pic_flag == 1`
//!   — the very first RBSP bit after the 2-byte NAL header.
//!
//! An AU therefore = the first NAL start code through the last slice of
//! one coded picture, grouping VPS+SPS+PPS+IDR into the keyframe AU so
//! the IDR is never stranded without its parameter sets. Mirrors the
//! SDK sample's `HelperH265FileParser`.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct H265Au<'a> {
    pub data: &'a [u8],
    /// True only when the AU is a self-contained sync point: an IRAP
    /// picture *together with* the VPS/SPS/PPS needed to decode it
    /// (the H.265 analog of h264's `is_key && sps && pps`).
    pub is_keyframe: bool,
}

/// Try to read one access-unit. `Ok(Some(au))` when a complete AU is
/// available (the next AU's start has been seen, or `eof`), `Ok(None)`
/// if more data is needed. Caller drains `au.data.len()`.
pub fn next_au(buf: &[u8], eof: bool) -> Result<Option<H265Au<'_>>, &'static str> {
    let starts = find_start_codes(buf);
    if starts.is_empty() {
        return if eof && !buf.is_empty() {
            Err("trailing bytes without H.265 start code")
        } else {
            Ok(None)
        };
    }
    let start = starts[0]; // include any prefixed VPS/SPS/PPS/SEI from the front
    let mut seen_vcl = false;
    let mut saw_vps = false;
    let mut saw_sps = false;
    let mut saw_pps = false;
    let mut saw_irap = false;

    for &p in &starts {
        let t = nal_unit_type_at(buf, p);
        if !seen_vcl {
            match t {
                32 => saw_vps = true,
                33 => saw_sps = true,
                34 => saw_pps = true,
                _ => {}
            }
            if is_vcl(t) {
                seen_vcl = true;
                if is_irap(t) {
                    saw_irap = true;
                }
            }
            continue;
        }
        // First VCL slice already seen: this NAL either continues the
        // current picture (a slice with first_slice_segment_in_pic_flag
        // == 0) or starts a new access unit.
        if is_au_delimiter_nal(t) {
            return Ok(Some(H265Au {
                data: &buf[start..p],
                is_keyframe: saw_irap && saw_vps && saw_sps && saw_pps,
            }));
        }
        if is_vcl(t) {
            match first_slice_in_pic_at(buf, p) {
                None => {
                    // Slice header not fully buffered yet.
                    if eof {
                        break;
                    }
                    return Ok(None);
                }
                Some(true) => {
                    return Ok(Some(H265Au {
                        data: &buf[start..p],
                        is_keyframe: saw_irap && saw_vps && saw_sps && saw_pps,
                    }));
                }
                Some(false) => {
                    if is_irap(t) {
                        saw_irap = true; // continuation slice of an IRAP picture
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
    Ok(Some(H265Au {
        data: &buf[start..],
        is_keyframe: saw_irap && saw_vps && saw_sps && saw_pps,
    }))
}

fn is_vcl(t: u8) -> bool {
    t <= 31
}

fn is_irap(t: u8) -> bool {
    (16..=23).contains(&t)
}

/// Non-VCL NAL types that may only appear *before* the first slice of a
/// picture; encountering one after a slice means a new AU has started.
fn is_au_delimiter_nal(t: u8) -> bool {
    matches!(t, 32 | 33 | 34 | 35 | 39) // VPS / SPS / PPS / AUD / prefix-SEI
}

/// Read `first_slice_segment_in_pic_flag` (the first RBSP bit after the
/// 2-byte NAL header) of the VCL NAL whose start code begins at `p`.
/// `None` if that byte isn't buffered yet. The flag cannot be reached
/// through an emulation-prevention byte (those only follow `00 00`
/// inside the RBSP), so a direct read is correct.
fn first_slice_in_pic_at(buf: &[u8], p: usize) -> Option<bool> {
    let sc_len = if buf.get(p..p + 4) == Some(&[0, 0, 0, 1]) { 4 } else { 3 };
    let idx = p + sc_len + 2; // skip start code + 2-byte NAL header
    buf.get(idx).map(|b| (b >> 7) & 1 == 1)
}

/// Byte offsets in `buf` where a 3- or 4-byte start code BEGINS.
fn find_start_codes(buf: &[u8]) -> Vec<usize> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 3 <= buf.len() {
        if buf[i] == 0 && buf[i + 1] == 0 {
            if i + 4 <= buf.len() && buf[i + 2] == 0 && buf[i + 3] == 1 {
                out.push(i);
                i += 4;
                continue;
            }
            if buf[i + 2] == 1 {
                out.push(i);
                i += 3;
                continue;
            }
        }
        i += 1;
    }
    out
}

fn nal_unit_type_at(buf: &[u8], start: usize) -> u8 {
    let header_off = if buf.get(start..start + 4) == Some(&[0, 0, 0, 1]) {
        start + 4
    } else {
        start + 3
    };
    buf.get(header_off).map(|b| (b >> 1) & 0x3F).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sc4() -> Vec<u8> {
        vec![0, 0, 0, 1]
    }

    /// VCL NALU: 4-byte start code + 2-byte HEVC NAL header
    /// (byte0 = nal_type<<1, byte1 = 0x01 → layer 0, tid 0) + RBSP whose
    /// first byte's top bit is `first_slice_segment_in_pic_flag`.
    /// `rbsp_len` is the RBSP byte count (>=1).
    fn vcl(nal_type: u8, first_in_pic: bool, rbsp_len: usize) -> Vec<u8> {
        let mut v = sc4();
        v.push(nal_type << 1);
        v.push(0x01);
        v.push(if first_in_pic { 0x80 } else { 0x00 });
        v.extend(std::iter::repeat(0xAB).take(rbsp_len.saturating_sub(1)));
        v
    }

    /// Non-VCL NALU (VPS/SPS/PPS/AUD/SEI): start code + 2-byte header + payload.
    fn nonvcl(nal_type: u8, len: usize) -> Vec<u8> {
        let mut v = sc4();
        v.push(nal_type << 1);
        v.push(0x01);
        v.extend(std::iter::repeat(0xAB).take(len));
        v
    }

    #[test]
    fn bare_idr_is_not_a_keyframe_without_param_sets() {
        let buf = vcl(19, true, 100); // IDR_W_RADL, no VPS/SPS/PPS
        let au = next_au(&buf, true).unwrap().unwrap();
        assert_eq!(au.data.len(), buf.len());
        assert!(!au.is_keyframe);
    }

    #[test]
    fn vps_sps_pps_idr_collapsed_into_one_keyframe_au() {
        let mut buf = nonvcl(32, 4); // VPS
        buf.extend(nonvcl(33, 5)); // SPS
        buf.extend(nonvcl(34, 3)); // PPS
        buf.extend(vcl(19, true, 50)); // IDR
        let au = next_au(&buf, true).unwrap().unwrap();
        assert_eq!(au.data.len(), buf.len());
        assert!(au.is_keyframe);
    }

    #[test]
    fn keyframe_param_sets_not_glued_onto_preceding_delta() {
        // [TRAIL][VPS][SPS][PPS][IDR][TRAIL] must split as
        // [TRAIL] then [VPS][SPS][PPS][IDR] (not [TRAIL][VPS][SPS][PPS]).
        let trail0 = vcl(1, true, 20); // TRAIL_R, first slice of its picture
        let vps = nonvcl(32, 4);
        let sps = nonvcl(33, 5);
        let pps = nonvcl(34, 3);
        let idr = vcl(19, true, 40);
        let trail1 = vcl(1, true, 10); // following picture (boundary marker)

        let mut buf = trail0.clone();
        buf.extend(&vps);
        buf.extend(&sps);
        buf.extend(&pps);
        buf.extend(&idr);
        buf.extend(&trail1);

        let au = next_au(&buf, false).unwrap().unwrap();
        assert_eq!(au.data.len(), trail0.len(), "first AU = lone TRAIL");
        assert!(!au.is_keyframe);

        let rest = &buf[trail0.len()..];
        let au2 = next_au(rest, false).unwrap().unwrap();
        assert_eq!(
            au2.data.len(),
            vps.len() + sps.len() + pps.len() + idr.len(),
            "second AU = VPS+SPS+PPS+IDR, ends before the trailing TRAIL"
        );
        assert!(au2.is_keyframe, "IDR must carry its param sets");
    }

    #[test]
    fn consecutive_delta_frames_split_each_picture() {
        let first = vcl(1, true, 30);
        let mut buf = first.clone();
        buf.extend(vcl(1, true, 20));
        let au = next_au(&buf, false).unwrap().unwrap();
        assert_eq!(au.data.len(), first.len());
        assert!(!au.is_keyframe);
    }

    #[test]
    fn multi_slice_picture_stays_one_au() {
        // One picture, two slices: slice0 first_in_pic=1, slice1=0.
        // Next picture's slice0 first_in_pic=1 marks the boundary.
        let s0 = vcl(1, true, 20);
        let s1 = vcl(1, false, 20);
        let mut buf = s0.clone();
        buf.extend(&s1);
        buf.extend(vcl(1, true, 10)); // next picture
        let au = next_au(&buf, false).unwrap().unwrap();
        assert_eq!(au.data.len(), s0.len() + s1.len(), "both slices in one AU");
        assert!(!au.is_keyframe);
    }

    #[test]
    fn returns_none_when_need_more() {
        let buf = vcl(1, true, 30);
        assert!(next_au(&buf, false).unwrap().is_none());
    }

    #[test]
    fn finds_3byte_and_4byte_start_codes() {
        // 3-byte start code IDR (type 19), then 4-byte start code TRAIL.
        let mut buf = vec![0, 0, 1, 19 << 1, 0x01, 0x80];
        buf.extend([0xAB; 20]);
        buf.extend([0, 0, 0, 1, 1 << 1, 0x01, 0x80]);
        buf.extend([0xAB; 20]);
        let au = next_au(&buf, false).unwrap().unwrap();
        assert!(!au.is_keyframe); // bare IDR, no param sets
        assert_eq!(au.data.len(), 3 + 3 + 20);
    }

    #[test]
    fn empty_returns_none() {
        assert!(next_au(&[], true).unwrap().is_none());
    }

    #[test]
    fn aud_and_sei_before_slice_join_the_keyframe_au() {
        // AUD(35) + prefix-SEI(39) + VPS + SPS + PPS + IDR → one keyframe AU.
        let mut buf = nonvcl(35, 2); // AUD
        buf.extend(nonvcl(39, 6)); // prefix SEI
        buf.extend(nonvcl(32, 4)); // VPS
        buf.extend(nonvcl(33, 5)); // SPS
        buf.extend(nonvcl(34, 3)); // PPS
        buf.extend(vcl(19, true, 40)); // IDR
        let au = next_au(&buf, true).unwrap().unwrap();
        assert_eq!(au.data.len(), buf.len());
        assert!(au.is_keyframe);
    }

    #[test]
    fn suffix_sei_after_slice_does_not_split() {
        // Suffix-SEI (40) belongs to the same AU as the preceding slice;
        // it is NOT an AU delimiter. Boundary is the next picture.
        let p0 = {
            let mut v = vcl(1, true, 20);
            v.extend(nonvcl(40, 4)); // suffix SEI, same AU
            v
        };
        let mut buf = p0.clone();
        buf.extend(vcl(1, true, 10)); // next picture
        let au = next_au(&buf, false).unwrap().unwrap();
        assert_eq!(au.data.len(), p0.len(), "suffix SEI stays with its slice");
        assert!(!au.is_keyframe);
    }

    #[test]
    fn cra_is_a_keyframe_with_param_sets() {
        let mut buf = nonvcl(32, 3); // VPS
        buf.extend(nonvcl(33, 4)); // SPS
        buf.extend(nonvcl(34, 2)); // PPS
        buf.extend(vcl(21, true, 30)); // CRA_NUT (IRAP)
        let au = next_au(&buf, true).unwrap().unwrap();
        assert_eq!(au.data.len(), buf.len());
        assert!(au.is_keyframe);
    }

    #[test]
    fn prefix_sei_after_slice_starts_new_au() {
        let trail = vcl(1, true, 15);
        let mut buf = trail.clone();
        buf.extend(nonvcl(39, 5)); // prefix SEI of the NEXT AU
        buf.extend(vcl(1, true, 10));
        let au = next_au(&buf, false).unwrap().unwrap();
        assert_eq!(au.data.len(), trail.len());
        assert!(!au.is_keyframe);
    }

    #[test]
    fn eof_flushes_final_picture() {
        // Last picture in the stream has no following start code; eof
        // must emit it.
        let mut buf = nonvcl(32, 3);
        buf.extend(nonvcl(33, 4));
        buf.extend(nonvcl(34, 2));
        buf.extend(vcl(20, true, 25)); // IDR_N_LP
        let total = buf.len();
        assert!(next_au(&buf, false).unwrap().is_none(), "no boundary yet");
        let au = next_au(&buf, true).unwrap().unwrap();
        assert_eq!(au.data.len(), total);
        assert!(au.is_keyframe);
    }

    #[test]
    fn trailing_bytes_without_start_code_error_on_eof() {
        assert!(next_au(&[0xAA, 0xBB, 0xCC, 0xDD], true).is_err());
    }

    /// Local cross-check against the SDK's own helper_h265_parser, using a
    /// real ffmpeg `-f hevc` bitstream. Inert unless `STA_REAL_HEVC` (input
    /// path) and `STA_REAL_HEVC_OUT` (where to write the frame table) are
    /// set. Run: `STA_REAL_HEVC=/tmp/t.hevc STA_REAL_HEVC_OUT=/tmp/rust_h265.txt
    /// cargo test --bins crosscheck_real_hevc`.
    #[test]
    fn crosscheck_real_hevc() {
        let (Ok(inp), Ok(outp)) = (
            std::env::var("STA_REAL_HEVC"),
            std::env::var("STA_REAL_HEVC_OUT"),
        ) else {
            return;
        };
        let data = std::fs::read(&inp).expect("read STA_REAL_HEVC");
        let mut out = String::new();
        let mut off = 0usize;
        let mut idx = 0usize;
        while off < data.len() {
            match next_au(&data[off..], true).unwrap() {
                None => break,
                Some(au) => {
                    out.push_str(&format!(
                        "{} {} {}\n",
                        idx,
                        au.data.len(),
                        if au.is_keyframe { 1 } else { 0 }
                    ));
                    off += au.data.len();
                    idx += 1;
                }
            }
        }
        std::fs::write(&outp, out).expect("write STA_REAL_HEVC_OUT");
        eprintln!("rust hevc frames={idx}");
    }
}
