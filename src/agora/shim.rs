//! FFI declarations for the C++ encoded-sender shim (`cpp/agora_shim.cpp`).
//!
//! The SDK's flat C `agora_video_encoded_image_sender_send` /
//! `agora_audio_encoded_frame_sender_send` are broken in this build —
//! the SDK accepts the first call, then rejects every subsequent call
//! (rc=1). The C++ method `IVideoEncodedImageSender::sendEncodedVideoImage`
//! works correctly (verified by running the SDK's own
//! `sample_send_h264_pcm` with return-value logging: 90/90 frames accepted).
//!
//! The shim links against the SDK's C++ headers and forwards through the
//! C++ vtable. See `cpp/agora_shim.cpp` / `cpp/agora_shim.h`.

#![allow(non_camel_case_types)]

use std::os::raw::c_void;

// Opaque handle types — Rust never dereferences these.
#[repr(C)]
pub struct cppshim_video_pub {
    _private: [u8; 0],
}
#[repr(C)]
pub struct cppshim_audio_pub {
    _private: [u8; 0],
}
#[repr(C)]
pub struct cppshim_local_user_observer {
    _private: [u8; 0],
}

unsafe extern "C" {
    pub fn cppshim_local_user_observer_register(c_conn_handle: *mut c_void) -> *mut cppshim_local_user_observer;
    pub fn cppshim_local_user_observer_destroy(obs: *mut cppshim_local_user_observer);

    pub fn cppshim_video_encoded_create(
        c_service_handle: *mut c_void,
        c_factory_handle: *mut c_void,
        codec_type: i32,
    ) -> *mut cppshim_video_pub;
    pub fn cppshim_video_encoded_send(
        p: *mut cppshim_video_pub,
        buf: *const u8,
        len: u32,
        is_keyframe: i32,
        fps: i32,
        codec_type: i32,
        width: i32,
        height: i32,
        capture_time_ms: i64,
    ) -> i32;
    pub fn cppshim_video_encoded_publish(p: *mut cppshim_video_pub, c_conn_handle: *mut c_void) -> i32;
    pub fn cppshim_video_encoded_unpublish(p: *mut cppshim_video_pub, c_conn_handle: *mut c_void) -> i32;
    pub fn cppshim_video_encoded_destroy(p: *mut cppshim_video_pub);

    pub fn cppshim_audio_encoded_create(
        c_service_handle: *mut c_void,
        c_factory_handle: *mut c_void,
        codec: i32,
    ) -> *mut cppshim_audio_pub;
    pub fn cppshim_audio_encoded_send(
        p: *mut cppshim_audio_pub,
        buf: *const u8,
        len: u32,
        codec: i32,
        sample_rate: i32,
        samples_per_channel: i32,
        channels: i32,
    ) -> i32;
    pub fn cppshim_audio_encoded_publish(p: *mut cppshim_audio_pub, c_conn_handle: *mut c_void) -> i32;
    pub fn cppshim_audio_encoded_unpublish(p: *mut cppshim_audio_pub, c_conn_handle: *mut c_void) -> i32;
    pub fn cppshim_audio_encoded_destroy(p: *mut cppshim_audio_pub);
}
