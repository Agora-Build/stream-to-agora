//! Frame-size helper for yuv420p. One frame = `width * height * 3 / 2`
//! bytes (Y plane: w*h, U plane: w*h/4, V plane: w*h/4 — planar,
//! contiguous). The raw-video pump reads exactly this many bytes from
//! ffmpeg's `-f rawvideo` stdout per frame; no in-band framing.

/// Required byte count for one yuv420p frame.
pub fn frame_bytes(width: u32, height: u32) -> usize {
    (width as usize * height as usize * 3) / 2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yuv420p_320x180_size() {
        assert_eq!(frame_bytes(320, 180), 320 * 180 * 3 / 2);
    }
}
