// C ABI for the C++ shim. Rust calls into these functions; they hold
// agora_refptr<> via opaque structs and forward to the C++ encoded-sender
// virtuals. Used because the SDK's flat C API for encoded senders is
// broken on this build — see README §Known Issues.
#pragma once
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

// Opaque handle types.
typedef struct cppshim_video_pub cppshim_video_pub;
typedef struct cppshim_audio_pub cppshim_audio_pub;

// Create an encoded-video publisher (sender + custom track).
//
// `c_service_handle`, `c_factory_handle` are the flat-C handles we already
// hold from `agora_service_create` / `agora_service_create_media_node_factory`.
// The shim derefs them to recover the underlying C++ object pointers (the
// flat-C wrappers are `void**` to a refptr block — verified empirically and
// via disassembly of `agora_video_encoded_image_sender_send`).
//
// `codec_type` is the VIDEO_CODEC_TYPE int (H264=2, H265=3, etc.). Defaults
// to H264 if 0 is passed.
//
// Returns NULL on failure.
cppshim_video_pub* cppshim_video_encoded_create(
    void* c_service_handle,
    void* c_factory_handle,
    int codec_type);

// Push one encoded video access unit. `is_keyframe` selects KEY (3) or
// DELTA (4) frame type. `fps` populates `framesPerSecond`. Returns 0 on
// success, non-zero on SDK rejection.
int cppshim_video_encoded_send(
    cppshim_video_pub* p,
    const uint8_t* buf,
    uint32_t len,
    int is_keyframe,
    int fps);

// Publish / unpublish the wrapped track on the given connection. The
// `c_conn_handle` is the flat-C handle from `agora_rtc_conn_create`.
// The shim calls `conn->getLocalUser()->publishVideo(track)` internally.
int cppshim_video_encoded_publish(cppshim_video_pub* p, void* c_conn_handle);
int cppshim_video_encoded_unpublish(cppshim_video_pub* p, void* c_conn_handle);

// Destroy the publisher. Releases the refptrs so the sender + track go away.
void cppshim_video_encoded_destroy(cppshim_video_pub* p);

// --- Audio ---

// Create an encoded-audio publisher (AAC sender + custom track).
cppshim_audio_pub* cppshim_audio_encoded_create(
    void* c_service_handle,
    void* c_factory_handle);

// Push one AAC frame (raw AAC bytes — typically an ADTS frame, but the
// SDK derives codec config from the bitstream itself).
//
// `samples_per_channel` is 1024 for AAC-LC (the SDK uses this to advance
// its internal timestamp); pass 0 to let the SDK compute it.
int cppshim_audio_encoded_send_aac(
    cppshim_audio_pub* p,
    const uint8_t* buf,
    uint32_t len,
    int sample_rate,
    int samples_per_channel,
    int channels);

int cppshim_audio_encoded_publish(cppshim_audio_pub* p, void* c_conn_handle);
int cppshim_audio_encoded_unpublish(cppshim_audio_pub* p, void* c_conn_handle);

void cppshim_audio_encoded_destroy(cppshim_audio_pub* p);

#ifdef __cplusplus
}
#endif
