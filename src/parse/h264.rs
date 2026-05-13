//! Annex-B NALU walker for H.264. Yields one access-unit per call.
//!
//! Simplification: ffmpeg's `-f h264` output writes each VCL NALU
//! prefixed by any parameter sets (SPS/PPS/SEI) for its picture,
//! separated by 4-byte start codes. We treat an AU as "all bytes from
//! one VCL NALU's start code up to (but not including) the *next* VCL
//! NALU's start code." That matches what
//! `agora_video_encoded_image_sender_send` expects per frame.
//!
//! VCL NALU types: 1 (non-IDR slice), 5 (IDR slice). Non-VCL types
//! include 6 (SEI), 7 (SPS), 8 (PPS), 9 (AUD), etc.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct H264Au<'a> {
    pub data: &'a [u8],
    pub is_keyframe: bool,    // true if any NALU in the AU is type 5 (IDR)
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
    // First VCL NALU position.
    let first_vcl = starts.iter().position(|&p| {
        let nal_type = nal_unit_type_at(buf, p);
        nal_type == 1 || nal_type == 5
    });
    let first_vcl = match first_vcl {
        Some(i) => i,
        None => return Ok(None), // no VCL yet; wait for more
    };
    // Second VCL NALU position (after first_vcl) — boundary of the AU.
    let second_vcl_offset = starts.iter().skip(first_vcl + 1).position(|&p| {
        let nal_type = nal_unit_type_at(buf, p);
        nal_type == 1 || nal_type == 5
    });
    let end = match second_vcl_offset {
        Some(off) => starts[first_vcl + 1 + off],
        None if eof => buf.len(),
        None => return Ok(None), // need more bytes to know where this AU ends
    };
    let start = starts[0]; // include any prefixed SPS/PPS/SEI from the very front
    let au = &buf[start..end];
    let is_keyframe = starts.iter().any(|&p| {
        p >= start && p < end && nal_unit_type_at(buf, p) == 5
    });
    Ok(Some(H264Au { data: au, is_keyframe }))
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
    /// Build a single NALU: start code + (header byte) + payload of 0xAB bytes.
    fn vcl(nal_type: u8, len: usize) -> Vec<u8> {
        let mut v = sc4();
        v.push(nal_type & 0x1F);
        v.extend(std::iter::repeat(0xAB).take(len.saturating_sub(1)));
        v
    }

    #[test]
    fn one_idr_au_with_eof() {
        let buf = vcl(5, 100);
        let au = next_au(&buf, true).unwrap().unwrap();
        assert_eq!(au.data.len(), buf.len());
        assert!(au.is_keyframe);
    }

    #[test]
    fn sps_pps_idr_collapsed_into_one_au() {
        let mut buf = vcl(7, 5);
        buf.extend(vcl(8, 5));
        buf.extend(vcl(5, 50));
        let au = next_au(&buf, true).unwrap().unwrap();
        assert_eq!(au.data.len(), buf.len());
        assert!(au.is_keyframe);
    }

    #[test]
    fn two_aus_split_at_second_vcl() {
        let mut buf = vcl(5, 30);
        let cutoff = buf.len();
        buf.extend(vcl(1, 20));
        let au = next_au(&buf, false).unwrap().unwrap();
        assert_eq!(au.data.len(), cutoff);
        assert!(au.is_keyframe);
    }

    #[test]
    fn returns_none_when_need_more() {
        let buf = vcl(1, 30);
        assert!(next_au(&buf, false).unwrap().is_none());
    }

    #[test]
    fn finds_3byte_and_4byte_start_codes() {
        let mut buf = vec![0,0,1, 0x65];
        buf.extend([0xAB; 20]);
        buf.extend([0,0,0,1, 0x41]);
        buf.extend([0xAB; 20]);
        let au = next_au(&buf, false).unwrap().unwrap();
        assert!(au.is_keyframe);
    }

    #[test]
    fn empty_returns_none() {
        assert!(next_au(&[], true).unwrap().is_none());
    }
}
