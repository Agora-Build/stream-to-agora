//! Ogg-Opus depacketiser: ffmpeg `-f ogg` wraps the copied Opus stream in
//! Ogg pages; the Agora encoded-audio sender wants the bare Opus packets
//! (TOC byte + frames), exactly like the SDK's `sample_send_opus` which
//! de-Oggs with libopusfile. We reconstruct packets from Ogg pages here.
//!
//! Per Ogg: a page is `"OggS"`, version, header-type, granule(8),
//! serial(4), seq(4), CRC(4), n_segments(1), segment-table(n), then the
//! segment data. A packet is the concatenation of consecutive segments
//! ending at a segment whose lacing value is `< 255`; a trailing `255`
//! lacing means the packet continues on the next (continued-flagged)
//! page. The first two packets are the `OpusHead` (channel count at byte
//! 9) and `OpusTags` headers — skipped. Every later packet is one Opus
//! packet; its TOC byte gives the sample count (for `samplesPerChannel`
//! and pacing). The SDK is always told `sampleRateHz = 48000` (Opus is
//! coded at 48 kHz regardless of the original input rate).
//!
//! CRC is not validated: the producer is a trusted local ffmpeg.

use std::collections::VecDeque;

/// One Opus packet ready to hand to `sendEncodedAudioFrame`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpusPacket {
    pub data: Vec<u8>,
    pub sample_rate: u32, // always 48000 for the SDK
    pub channels: u32,
    pub samples_per_channel: u32,
}

/// Stateful Ogg-Opus reader. Owns cross-page reassembly + a small queue
/// of decoded packets, and drains consumed Ogg pages from the caller's
/// buffer itself (Ogg framing is page-granular, unlike ADTS).
pub struct OpusDemux {
    channels: u32,
    carry: Vec<u8>,
    carry_active: bool,
    ready: VecDeque<OpusPacket>,
}

impl Default for OpusDemux {
    fn default() -> Self {
        Self::new()
    }
}

impl OpusDemux {
    pub fn new() -> Self {
        OpusDemux {
            channels: 0,
            carry: Vec::new(),
            carry_active: false,
            ready: VecDeque::new(),
        }
    }

    /// Pull the next Opus audio packet, consuming whole Ogg pages from
    /// the front of `buf` as needed. `Ok(Some(pkt))` when one is ready,
    /// `Ok(None)` if more bytes are needed (or stream end), `Err` on a
    /// malformed Opus packet.
    pub fn pull(&mut self, buf: &mut Vec<u8>) -> Result<Option<OpusPacket>, &'static str> {
        loop {
            if let Some(p) = self.ready.pop_front() {
                return Ok(Some(p));
            }
            // Resync to a page boundary (only relevant after a respawn /
            // leading garbage; steady state is already page-aligned).
            match find_subslice(buf, b"OggS") {
                None => return Ok(None),
                Some(0) => {}
                Some(pos) => {
                    buf.drain(0..pos);
                }
            }
            if buf.len() < 27 {
                return Ok(None);
            }
            let nseg = buf[26] as usize;
            if buf.len() < 27 + nseg {
                return Ok(None);
            }
            let data_len: usize = buf[27..27 + nseg].iter().map(|&b| b as usize).sum();
            let page_total = 27 + nseg + data_len;
            if buf.len() < page_total {
                return Ok(None);
            }
            let continued = buf[5] & 0x01 != 0;
            let seg_table: Vec<u8> = buf[27..27 + nseg].to_vec();
            let data: Vec<u8> = buf[27 + nseg..page_total].to_vec();
            buf.drain(0..page_total);

            let mut packets: Vec<Vec<u8>> = Vec::new();
            let mut cur: Vec<u8> = if continued && self.carry_active {
                std::mem::take(&mut self.carry)
            } else {
                Vec::new()
            };
            self.carry_active = false;
            let mut off = 0usize;
            let mut last_was_255 = false;
            for &lace in &seg_table {
                let l = lace as usize;
                cur.extend_from_slice(&data[off..off + l]);
                off += l;
                last_was_255 = lace == 255;
                if lace < 255 {
                    packets.push(std::mem::take(&mut cur));
                }
            }
            if last_was_255 {
                self.carry = std::mem::take(&mut cur);
                self.carry_active = true;
            }

            for pkt in packets {
                if pkt.is_empty() {
                    continue;
                }
                if pkt.len() >= 10 && &pkt[0..8] == b"OpusHead" {
                    self.channels = pkt[9] as u32;
                    continue;
                }
                if pkt.len() >= 8 && &pkt[0..8] == b"OpusTags" {
                    continue;
                }
                let spc = opus_samples(&pkt)?;
                let channels = if self.channels == 0 { 2 } else { self.channels };
                self.ready.push_back(OpusPacket {
                    data: pkt,
                    sample_rate: 48000,
                    channels,
                    samples_per_channel: spc,
                });
            }
            // Loop: serve a queued packet, or parse the next page.
        }
    }
}

/// Samples-per-channel for one Opus packet, from its TOC byte (RFC 6716
/// §3.1). All frames in a packet share one duration; total = nframes ×
/// frame-samples @ 48 kHz.
fn opus_samples(pkt: &[u8]) -> Result<u32, &'static str> {
    let toc = pkt[0];
    let config = (toc >> 3) as usize; // 0..=31
    let code = toc & 0x03;
    let nframes: u32 = match code {
        0 => 1,
        1 | 2 => 2,
        _ => {
            if pkt.len() < 2 {
                return Err("opus code-3 packet truncated");
            }
            (pkt[1] & 0x3F) as u32
        }
    };
    if nframes == 0 {
        return Err("opus packet declares 0 frames");
    }
    // Frame duration in hundredths of a millisecond.
    let dur_x100: u32 = if config < 12 {
        [1000, 2000, 4000, 6000][config % 4] // SILK NB/MB/WB: 10/20/40/60 ms
    } else if config < 16 {
        [1000, 2000][(config - 12) % 2] // Hybrid SWB/FB: 10/20 ms
    } else {
        [250, 500, 1000, 2000][(config - 16) % 4] // CELT: 2.5/5/10/20 ms
    };
    Ok(nframes * (48000 * dur_x100 / 100_000))
}

fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ogg_page(header_type: u8, seq: u32, segs: &[u8], data: &[u8]) -> Vec<u8> {
        let mut v = b"OggS".to_vec();
        v.push(0); // version
        v.push(header_type);
        v.extend(0u64.to_le_bytes()); // granule
        v.extend(1u32.to_le_bytes()); // serial
        v.extend(seq.to_le_bytes()); // page seq
        v.extend([0u8; 4]); // CRC (not validated)
        v.push(segs.len() as u8);
        v.extend(segs);
        v.extend(data);
        v
    }

    fn opus_head(channels: u8) -> Vec<u8> {
        let mut v = b"OpusHead".to_vec();
        v.push(1); // version (index 8)
        v.push(channels); // channel count (index 9)
        v.extend([0u8; 9]); // preskip(2)+inrate(4)+gain(2)+mapping(1)
        v
    }
    fn opus_tags() -> Vec<u8> {
        let mut v = b"OpusTags".to_vec();
        v.extend([0u8; 8]); // vendor len 0 + comment count 0
        v
    }
    /// Audio packet: TOC for config 31 (CELT FB), code = `code`.
    fn audio_pkt(code: u8, frame_count: u8, body: usize) -> Vec<u8> {
        let toc = (31u8 << 3) | (code & 0x03);
        let mut v = vec![toc];
        if code == 3 {
            v.push(frame_count & 0x3F);
        }
        v.extend(std::iter::repeat(0xAA).take(body));
        v
    }

    fn header_pages() -> Vec<u8> {
        let h = opus_head(2);
        let t = opus_tags();
        let mut s = ogg_page(0x02, 0, &[h.len() as u8], &h);
        s.extend(ogg_page(0x00, 1, &[t.len() as u8], &t));
        s
    }

    #[test]
    fn yields_audio_packets_and_skips_headers() {
        let a = audio_pkt(0, 0, 7); // len 8, config 31 code 0 → 960 spc
        let b = audio_pkt(0, 0, 4); // len 5
        let mut stream = header_pages();
        stream.extend(ogg_page(
            0x00,
            2,
            &[a.len() as u8, b.len() as u8],
            &[a.clone(), b.clone()].concat(),
        ));
        let mut d = OpusDemux::new();
        let mut buf = stream;

        let p1 = d.pull(&mut buf).unwrap().unwrap();
        assert_eq!(p1.data, a);
        assert_eq!(p1.sample_rate, 48000);
        assert_eq!(p1.channels, 2);
        assert_eq!(p1.samples_per_channel, 960); // CELT FB 20 ms @ 48 k

        let p2 = d.pull(&mut buf).unwrap().unwrap();
        assert_eq!(p2.data, b);
        assert_eq!(p2.channels, 2);

        assert!(d.pull(&mut buf).unwrap().is_none());
    }

    #[test]
    fn cross_page_packet_reassembled() {
        // One 265-byte packet split: page A lacing [255] (continues),
        // page B continued-flag lacing [10].
        let mut pkt = audio_pkt(0, 0, 264); // total len 265
        assert_eq!(pkt.len(), 265);
        let head = pkt[..255].to_vec();
        let tail = pkt[255..].to_vec();
        let mut stream = header_pages();
        stream.extend(ogg_page(0x00, 2, &[255], &head));
        stream.extend(ogg_page(0x01, 3, &[10], &tail)); // 0x01 = continued
        let mut d = OpusDemux::new();
        let mut buf = stream;
        let p = d.pull(&mut buf).unwrap().unwrap();
        assert_eq!(p.data.len(), 265);
        pkt.truncate(265);
        assert_eq!(p.data, pkt);
        assert!(d.pull(&mut buf).unwrap().is_none());
    }

    #[test]
    fn need_more_when_partial_page() {
        let mut buf = b"OggS".to_vec();
        buf.extend([0u8; 10]); // truncated header
        let mut d = OpusDemux::new();
        assert!(d.pull(&mut buf).unwrap().is_none());
    }

    #[test]
    fn resync_skips_leading_garbage() {
        let a = audio_pkt(0, 0, 9);
        let mut stream = b"\xde\xad\xbe\xefJUNK".to_vec();
        stream.extend(header_pages());
        stream.extend(ogg_page(0x00, 2, &[a.len() as u8], &a));
        let mut d = OpusDemux::new();
        let mut buf = stream;
        let p = d.pull(&mut buf).unwrap().unwrap();
        assert_eq!(p.data, a);
    }

    #[test]
    fn toc_sample_counts() {
        // config 0 (SILK NB 10 ms) code 0 → 480 @ 48 k
        assert_eq!(opus_samples(&[(0u8 << 3) | 0, 0xAA]).unwrap(), 480);
        // config 31 (CELT FB 20 ms) code 0 → 960
        assert_eq!(opus_samples(&[(31u8 << 3) | 0]).unwrap(), 960);
        // config 31 code 3, 3 frames → 3 × 960 = 2880
        assert_eq!(opus_samples(&[(31u8 << 3) | 3, 3]).unwrap(), 2880);
        // config 16 (CELT NB 2.5 ms) code 0 → 120
        assert_eq!(opus_samples(&[(16u8 << 3) | 0]).unwrap(), 120);
        // code 3 truncated
        assert!(opus_samples(&[(31u8 << 3) | 3]).is_err());
        // code 1 / code 2 → 2 frames. config 31 (20 ms) → 2 × 960
        assert_eq!(opus_samples(&[(31u8 << 3) | 1]).unwrap(), 1920);
        assert_eq!(opus_samples(&[(31u8 << 3) | 2]).unwrap(), 1920);
        // code 3, 0 frames → error
        assert!(opus_samples(&[(31u8 << 3) | 3, 0]).is_err());
    }

    #[test]
    fn mono_channel_count_from_opushead() {
        let h = opus_head(1); // mono
        let t = opus_tags();
        let a = audio_pkt(0, 0, 6);
        let mut buf = ogg_page(0x02, 0, &[h.len() as u8], &h);
        buf.extend(ogg_page(0x00, 1, &[t.len() as u8], &t));
        buf.extend(ogg_page(0x00, 2, &[a.len() as u8], &a));
        let mut d = OpusDemux::new();
        let p = d.pull(&mut buf).unwrap().unwrap();
        assert_eq!(p.channels, 1);
    }

    #[test]
    fn audio_packets_across_multiple_pages() {
        let a = audio_pkt(0, 0, 6);
        let b = audio_pkt(1, 0, 9); // code 1 → 2 frames → 1920 spc
        let mut buf = header_pages();
        buf.extend(ogg_page(0x00, 2, &[a.len() as u8], &a));
        buf.extend(ogg_page(0x00, 3, &[b.len() as u8], &b));
        let mut d = OpusDemux::new();
        let p1 = d.pull(&mut buf).unwrap().unwrap();
        assert_eq!(p1.data, a);
        assert_eq!(p1.samples_per_channel, 960);
        let p2 = d.pull(&mut buf).unwrap().unwrap();
        assert_eq!(p2.data, b);
        assert_eq!(p2.samples_per_channel, 1920);
        assert!(d.pull(&mut buf).unwrap().is_none());
    }

    #[test]
    fn empty_page_between_audio_is_skipped() {
        let a = audio_pkt(0, 0, 5);
        let mut buf = header_pages();
        buf.extend(ogg_page(0x00, 2, &[], &[])); // nseg = 0, no packets
        buf.extend(ogg_page(0x00, 3, &[a.len() as u8], &a));
        let mut d = OpusDemux::new();
        let p = d.pull(&mut buf).unwrap().unwrap();
        assert_eq!(p.data, a);
    }

    /// Local validation against a real ffmpeg `-f ogg` Opus stream.
    /// Inert unless `STA_REAL_OPUS` (input) + `STA_REAL_OPUS_OUT` (report
    /// file) are set. Writes "<pkt_count> <total_samples> <channels>".
    #[test]
    fn crosscheck_real_opus() {
        let (Ok(inp), Ok(outp)) = (
            std::env::var("STA_REAL_OPUS"),
            std::env::var("STA_REAL_OPUS_OUT"),
        ) else {
            return;
        };
        let mut buf = std::fs::read(&inp).expect("read STA_REAL_OPUS");
        let mut d = OpusDemux::new();
        let mut count = 0u64;
        let mut total_samples = 0u64;
        let mut channels = 0u32;
        while let Some(p) = d.pull(&mut buf).unwrap() {
            count += 1;
            total_samples += p.samples_per_channel as u64;
            channels = p.channels;
            assert_eq!(p.sample_rate, 48000);
            assert!(!p.data.is_empty());
        }
        std::fs::write(&outp, format!("{count} {total_samples} {channels}\n"))
            .expect("write STA_REAL_OPUS_OUT");
        eprintln!("rust opus packets={count} samples={total_samples} ch={channels}");
    }

    #[test]
    fn incremental_feed_yields_packet_once_complete() {
        // Simulate the pump: bytes arrive in chunks; pull returns None
        // until a whole page is buffered, then yields the packet.
        let a = audio_pkt(0, 0, 7);
        let mut full = header_pages();
        full.extend(ogg_page(0x00, 2, &[a.len() as u8], &a));
        let mut d = OpusDemux::new();
        let mut buf: Vec<u8> = Vec::new();
        let mut got = None;
        for chunk in full.chunks(13) {
            buf.extend_from_slice(chunk);
            while let Some(p) = d.pull(&mut buf).unwrap() {
                got = Some(p);
            }
        }
        let p = got.expect("packet after full stream fed in 13-byte chunks");
        assert_eq!(p.data, a);
        assert_eq!(p.channels, 2);
    }
}
