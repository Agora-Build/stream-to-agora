//! Encoded + raw local audio publishers.
//!
//! The encoded path routes through the C++ shim (`cpp/agora_shim.cpp`)
//! because the SDK's flat-C `agora_audio_encoded_frame_sender_send` is
//! broken — see README §Known Issues. The raw path still uses the flat
//! C API directly.

use std::os::raw::c_void;
use std::ptr;

use super::error::{check, AgoraError};
use super::shim;
use super::sys;

/// AUDIO_CODEC_TYPE ints from the SDK's AgoraBase.h.
const AUDIO_CODEC_OPUS: i32 = 1;
const AUDIO_CODEC_PCMA: i32 = 3;
const AUDIO_CODEC_PCMU: i32 = 4;
const AUDIO_CODEC_AACLC: i32 = 8;
const AUDIO_CODEC_HEAAC: i32 = 9;
const AUDIO_CODEC_HEAAC2: i32 = 11;

/// Map ffprobe's `codec_name` (+ `profile` for the AAC family) to the
/// shim's AUDIO_CODEC_TYPE int.
///
/// HE-AAC note: ffmpeg's ADTS muxer uses *implicit* SBR signalling, so
/// the ADTS header carries the AAC core sample rate and one frame is
/// 1024 core samples. 1024/core == 2048/(2·core), i.e. the per-frame
/// duration is identical whether expressed at the core or the doubled
/// SBR rate — so `parse::aac`'s 1024-samples / core-rate values keep the
/// SDK's timestamp clock correct unchanged; only the codec id needs to
/// say HE-AAC so the SDK enables SBR decode.
fn audio_codec(codec_name: &str, profile: Option<&str>) -> i32 {
    match codec_name {
        "opus" => AUDIO_CODEC_OPUS,
        "pcm_mulaw" => AUDIO_CODEC_PCMU,
        "pcm_alaw" => AUDIO_CODEC_PCMA,
        "aac" => match profile {
            Some(p) if p.eq_ignore_ascii_case("HE-AACv2") => AUDIO_CODEC_HEAAC2,
            Some(p) if p.eq_ignore_ascii_case("HE-AAC") => AUDIO_CODEC_HEAAC,
            _ => AUDIO_CODEC_AACLC,
        },
        _ => AUDIO_CODEC_AACLC,
    }
}

pub struct EncodedAudioPublisher {
    shim: *mut shim::cppshim_audio_pub,
    conn: *mut c_void,
    /// AUDIO_CODEC_TYPE int forwarded per-frame (8 → AAC-LC, 1 → Opus).
    codec: i32,
}

// SAFETY: the shim pointer is opaque and never aliased across threads
// without external synchronisation; the pump task owns it exclusively.
unsafe impl Send for EncodedAudioPublisher {}

pub struct RawAudioPublisher {
    sender: *mut c_void,
    track: *mut c_void,
    conn: *mut c_void,
}

unsafe impl Send for RawAudioPublisher {}

pub(super) fn create_encoded(
    svc: *mut c_void,
    conn: *mut c_void,
    factory: *mut c_void,
    codec_name: &str,
    profile: Option<&str>,
) -> Result<EncodedAudioPublisher, AgoraError> {
    let codec = audio_codec(codec_name, profile);
    let p = unsafe { shim::cppshim_audio_encoded_create(svc, factory, codec) };
    if p.is_null() {
        return Err(AgoraError::null("cppshim_audio_encoded_create"));
    }
    Ok(EncodedAudioPublisher { shim: p, conn, codec })
}

pub(super) fn create_raw(
    svc: *mut c_void,
    conn: *mut c_void,
    factory: *mut c_void,
) -> Result<RawAudioPublisher, AgoraError> {
    let sender = unsafe {
        sys::agora_media_node_factory_create_audio_pcm_data_sender(factory)
    };
    if sender.is_null() {
        return Err(AgoraError::null(
            "agora_media_node_factory_create_audio_pcm_data_sender",
        ));
    }
    let track = unsafe { sys::agora_service_create_custom_audio_track_pcm(svc, sender) };
    if track.is_null() {
        unsafe {
            sys::agora_audio_pcm_data_sender_destroy(sender);
        }
        return Err(AgoraError::null(
            "agora_service_create_custom_audio_track_pcm",
        ));
    }
    unsafe {
        sys::agora_local_audio_track_set_enabled(track, 1);
    }
    Ok(RawAudioPublisher {
        sender,
        track,
        conn,
    })
}

impl EncodedAudioPublisher {
    /// Push one encoded audio frame — an ADTS-framed AAC frame or a bare
    /// Opus packet, per the codec this publisher was created for.
    pub fn push_audio(
        &self,
        frame: &[u8],
        sample_rate: u32,
        samples_per_channel: u32,
        channels: u32,
    ) -> Result<(), AgoraError> {
        let rc = unsafe {
            shim::cppshim_audio_encoded_send(
                self.shim,
                frame.as_ptr(),
                frame.len() as u32,
                self.codec,
                sample_rate as i32,
                samples_per_channel as i32,
                channels as i32,
            )
        };
        check(rc, "cppshim_audio_encoded_send")
    }

    pub fn publish(&self) -> Result<(), AgoraError> {
        let rc = unsafe { shim::cppshim_audio_encoded_publish(self.shim, self.conn) };
        check(rc, "cppshim_audio_encoded_publish")
    }

    pub fn unpublish(&self) -> Result<(), AgoraError> {
        let rc = unsafe { shim::cppshim_audio_encoded_unpublish(self.shim, self.conn) };
        check(rc, "cppshim_audio_encoded_unpublish")
    }
}

impl Drop for EncodedAudioPublisher {
    fn drop(&mut self) {
        if !self.shim.is_null() {
            unsafe {
                let _ = shim::cppshim_audio_encoded_unpublish(self.shim, self.conn);
                shim::cppshim_audio_encoded_destroy(self.shim);
            }
            self.shim = ptr::null_mut();
        }
    }
}

impl RawAudioPublisher {
    /// Push one PCM frame. `data` is `samples_per_channel * channels * 2`
    /// bytes of interleaved s16le. The SDK requires
    /// `samples_per_channel * 100 == sample_rate` for 10ms chunks.
    pub fn push_pcm(
        &self,
        data: &[u8],
        capture_timestamp_ms: u32,
        samples_per_channel: u32,
        channels: u32,
        sample_rate: u32,
    ) -> Result<(), AgoraError> {
        let bytes_per_sample = 2 * channels;
        let rc = unsafe {
            sys::agora_audio_pcm_data_sender_send(
                self.sender,
                data.as_ptr() as *const c_void,
                capture_timestamp_ms,
                samples_per_channel,
                bytes_per_sample,
                channels,
                sample_rate,
            )
        };
        check(rc, "agora_audio_pcm_data_sender_send")
    }

    pub fn publish(&self) -> Result<(), AgoraError> {
        let local = unsafe { sys::agora_rtc_conn_get_local_user(self.conn) };
        let rc = unsafe { sys::agora_local_user_publish_audio(local, self.track) };
        check(rc, "agora_local_user_publish_audio")
    }

    pub fn unpublish(&self) -> Result<(), AgoraError> {
        let local = unsafe { sys::agora_rtc_conn_get_local_user(self.conn) };
        let rc = unsafe { sys::agora_local_user_unpublish_audio(local, self.track) };
        check(rc, "agora_local_user_unpublish_audio")
    }
}

impl Drop for RawAudioPublisher {
    fn drop(&mut self) {
        unsafe {
            let local = sys::agora_rtc_conn_get_local_user(self.conn);
            let _ = sys::agora_local_user_unpublish_audio(local, self.track);
            sys::agora_local_audio_track_destroy(self.track);
            sys::agora_audio_pcm_data_sender_destroy(self.sender);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Same regression class as `video_codec_type`: these integers are
    /// the SDK's `AUDIO_CODEC_TYPE` enum (AgoraBase.h) and MUST match it,
    /// incl. the AAC profile → HE-AAC/HE-AACv2 disambiguation.
    #[test]
    fn audio_codec_maps_codec_and_profile() {
        assert_eq!(audio_codec("aac", None), 8, "AUDIO_CODEC_AACLC");
        assert_eq!(audio_codec("aac", Some("LC")), 8, "plain AAC-LC profile");
        assert_eq!(audio_codec("aac", Some("HE-AAC")), 9, "AUDIO_CODEC_HEAAC");
        assert_eq!(audio_codec("aac", Some("he-aac")), 9, "case-insensitive");
        assert_eq!(audio_codec("aac", Some("HE-AACv2")), 11, "AUDIO_CODEC_HEAAC2");
        assert_eq!(audio_codec("aac", Some("HE-AACV2")), 11, "case-insensitive");
        assert_eq!(audio_codec("opus", None), 1, "AUDIO_CODEC_OPUS");
        assert_eq!(audio_codec("pcm_mulaw", None), 4, "AUDIO_CODEC_PCMU");
        assert_eq!(audio_codec("pcm_alaw", None), 3, "AUDIO_CODEC_PCMA");
        assert_eq!(audio_codec("mp3", None), 8, "unknown → AAC-LC default");
    }
}
