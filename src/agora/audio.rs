//! Encoded + raw local audio publishers.

use std::os::raw::c_void;

use super::error::{check, AgoraError};
use super::sys;

// AUDIO_CODEC_TYPE values from AgoraBase.h.
const AUDIO_CODEC_AACLC: i32 = 8;

pub struct EncodedAudioPublisher {
    sender: *mut c_void,
    track: *mut c_void,
    conn: *mut c_void,
    codec: i32,
}

// SAFETY: the SDK handles are opaque C pointers that are not thread-local;
// we never alias them across threads without external synchronisation, and
// the callers in Task 11 keep the publishers in a single-threaded context.
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
    let sender = unsafe {
        sys::agora_media_node_factory_create_audio_encoded_frame_sender(factory)
    };
    if sender.is_null() {
        return Err(AgoraError::null(
            "agora_media_node_factory_create_audio_encoded_frame_sender",
        ));
    }
    // mix_mode: 0 = single mode (we're the only source for this track).
    let track = unsafe {
        sys::agora_service_create_custom_audio_track_encoded(svc, sender, 0)
    };
    if track.is_null() {
        unsafe {
            sys::agora_audio_encoded_frame_sender_destroy(sender);
        }
        return Err(AgoraError::null(
            "agora_service_create_custom_audio_track_encoded",
        ));
    }
    unsafe {
        sys::agora_local_audio_track_set_enabled(track, 1);
    }
    Ok(EncodedAudioPublisher {
        sender,
        track,
        conn,
        codec: AUDIO_CODEC_AACLC,
    })
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
    /// Push one AAC frame. `frame` is the ADTS-or-raw-AAC bytes from the
    /// caller's parser. `channels` is not part of the info struct (the SDK
    /// derives it from the codec config in the bitstream); we just ignore it.
    pub fn push_aac(
        &self,
        frame: &[u8],
        sample_rate: u32,
        samples_per_channel: u32,
        _channels: u32,
    ) -> Result<(), AgoraError> {
        let mut info: sys::encoded_audio_frame_info = unsafe { std::mem::zeroed() };
        info.speech = 0;
        info.codec = self.codec;
        info.sample_rate_hz = sample_rate as i32;
        info.samples_per_channel = samples_per_channel as i32;
        info.send_even_if_empty = 1;
        let rc = unsafe {
            sys::agora_audio_encoded_frame_sender_send(
                self.sender,
                frame.as_ptr() as *const _,
                frame.len() as u32,
                &info,
            )
        };
        check(rc, "agora_audio_encoded_frame_sender_send")
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

impl Drop for EncodedAudioPublisher {
    fn drop(&mut self) {
        unsafe {
            let local = sys::agora_rtc_conn_get_local_user(self.conn);
            let _ = sys::agora_local_user_unpublish_audio(local, self.track);
            sys::agora_local_audio_track_destroy(self.track);
            sys::agora_audio_encoded_frame_sender_destroy(self.sender);
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
