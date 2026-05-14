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

pub struct EncodedAudioPublisher {
    shim: *mut shim::cppshim_audio_pub,
    conn: *mut c_void,
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
) -> Result<EncodedAudioPublisher, AgoraError> {
    let p = unsafe { shim::cppshim_audio_encoded_create(svc, factory) };
    if p.is_null() {
        return Err(AgoraError::null("cppshim_audio_encoded_create"));
    }
    Ok(EncodedAudioPublisher { shim: p, conn })
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
    /// Push one AAC frame. `frame` is the ADTS-framed AAC bytes from the
    /// caller's parser.
    pub fn push_aac(
        &self,
        frame: &[u8],
        sample_rate: u32,
        samples_per_channel: u32,
        channels: u32,
    ) -> Result<(), AgoraError> {
        let rc = unsafe {
            shim::cppshim_audio_encoded_send_aac(
                self.shim,
                frame.as_ptr(),
                frame.len() as u32,
                sample_rate as i32,
                samples_per_channel as i32,
                channels as i32,
            )
        };
        check(rc, "cppshim_audio_encoded_send_aac")
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
