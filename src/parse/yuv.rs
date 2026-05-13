//! Fixed-size yuv420p frame reader. One frame = width * height * 3/2 bytes
//! (Y plane: w*h, U plane: w*h/4, V plane: w*h/4 — all planar, contiguous).

/// Required byte count for one yuv420p frame.
pub fn frame_bytes(width: u32, height: u32) -> usize {
    (width as usize * height as usize * 3) / 2
}

/// Try to read one frame from the front of `buf`. Returns `Ok(Some(slice))`
/// when at least `frame_bytes(width, height)` bytes are available, or
/// `Ok(None)` if more bytes are needed.
pub fn next_frame(buf: &[u8], width: u32, height: u32) -> Result<Option<&[u8]>, &'static str> {
    if width == 0 || height == 0 { return Err("zero dimensions"); }
    if width % 2 != 0 || height % 2 != 0 { return Err("yuv420p requires even dimensions"); }
    let need = frame_bytes(width, height);
    if buf.len() < need { Ok(None) } else { Ok(Some(&buf[..need])) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yuv420p_320x180_size() {
        assert_eq!(frame_bytes(320, 180), 320 * 180 * 3 / 2);
    }

    #[test]
    fn returns_none_when_short() {
        let buf = vec![0u8; 100];
        assert!(next_frame(&buf, 320, 180).unwrap().is_none());
    }

    #[test]
    fn returns_first_frame_when_full() {
        let buf = vec![0u8; frame_bytes(8, 8) * 2];
        let f = next_frame(&buf, 8, 8).unwrap().unwrap();
        assert_eq!(f.len(), frame_bytes(8, 8));
    }

    #[test]
    fn rejects_odd_dimensions() {
        let buf = vec![0u8; 1000];
        assert!(next_frame(&buf, 7, 8).is_err());
    }
}
