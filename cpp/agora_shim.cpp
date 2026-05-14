// C++ implementation of the encoded-sender shim. See agora_shim.h.
//
// The flat C API exported by libagora_rtc_sdk.so has a bug in
// `agora_video_encoded_image_sender_send` and
// `agora_audio_encoded_frame_sender_send`: the SDK accepts the first call,
// rejects all subsequent calls (rc=1). The C++ method
// `IVideoEncodedImageSender::sendEncodedVideoImage` (and the audio analog)
// works correctly — verified by running the SDK's own
// sample_send_h264_pcm with return-value logging (90/90 frames accepted).
//
// This shim links against the SDK's C++ headers and invokes the C++
// methods directly, avoiding the buggy C wrappers entirely.

#include "agora_shim.h"

#include "IAgoraService.h"
#include "NGIAgoraRtcConnection.h"
#include "NGIAgoraLocalUser.h"
#include "NGIAgoraMediaNodeFactory.h"
#include "NGIAgoraMediaNode.h"
#include "NGIAgoraVideoTrack.h"
#include "NGIAgoraAudioTrack.h"
#include "AgoraBase.h"

namespace ar = agora::rtc;
namespace ab = agora::base;

// Helper: a flat-C handle from `agora_*_create` is a pointer to a small
// struct whose first 8 bytes hold the underlying C++ object pointer (verified
// by disassembly of the C wrappers — they do `mov (handle), %rdi` before
// dispatching the C++ virtual). So `*(void**)handle` recovers it.
template <typename T>
static T* deref_c_handle(void* c_handle) {
    if (!c_handle) return nullptr;
    return *reinterpret_cast<T**>(c_handle);
}

// --- Video ---

struct cppshim_video_pub {
    agora::agora_refptr<ar::IVideoEncodedImageSender> sender;
    agora::agora_refptr<ar::ILocalVideoTrack> track;
};

extern "C" {

cppshim_video_pub* cppshim_video_encoded_create(
    void* c_service_handle,
    void* c_factory_handle,
    int codec_type) {
    auto* service = deref_c_handle<ab::IAgoraService>(c_service_handle);
    auto* factory = deref_c_handle<ar::IMediaNodeFactory>(c_factory_handle);
    if (!service || !factory) return nullptr;

    auto sender = factory->createVideoEncodedImageSender();
    if (!sender) return nullptr;

    ar::SenderOptions opts;
    // C++ defaults: ccMode=CC_ENABLED, codecType=H265, targetBitrate=6500.
    // The SDK's own sample uses the default codecType=H265 even when sending
    // H.264 — the per-frame EncodedVideoFrameInfo.codecType is what routes.
    // We still respect a non-zero `codec_type` argument for callers that
    // want to be explicit.
    if (codec_type != 0) {
        opts.codecType = static_cast<ar::VIDEO_CODEC_TYPE>(codec_type);
    }

    auto track = service->createCustomVideoTrack(sender, opts);
    if (!track) return nullptr;
    track->setEnabled(true);

    return new cppshim_video_pub{sender, track};
}

int cppshim_video_encoded_send(
    cppshim_video_pub* p,
    const uint8_t* buf,
    uint32_t len,
    int is_keyframe,
    int fps) {
    if (!p || !p->sender || !buf || len == 0) return -1;
    ar::EncodedVideoFrameInfo info;
    info.codecType = ar::VIDEO_CODEC_H264;
    info.framesPerSecond = fps;
    info.frameType = is_keyframe
        ? ar::VIDEO_FRAME_TYPE_KEY_FRAME
        : ar::VIDEO_FRAME_TYPE_DELTA_FRAME;
    info.rotation = ar::VIDEO_ORIENTATION_0;
    bool ok = p->sender->sendEncodedVideoImage(buf, len, info);
    return ok ? 0 : 1;
}

int cppshim_video_encoded_publish(cppshim_video_pub* p, void* c_conn_handle) {
    if (!p || !p->track) return -1;
    auto* conn = deref_c_handle<ar::IRtcConnection>(c_conn_handle);
    if (!conn) return -1;
    auto* local = conn->getLocalUser();
    if (!local) return -1;
    return local->publishVideo(p->track);
}

int cppshim_video_encoded_unpublish(cppshim_video_pub* p, void* c_conn_handle) {
    if (!p || !p->track) return -1;
    auto* conn = deref_c_handle<ar::IRtcConnection>(c_conn_handle);
    if (!conn) return -1;
    auto* local = conn->getLocalUser();
    if (!local) return -1;
    return local->unpublishVideo(p->track);
}

void cppshim_video_encoded_destroy(cppshim_video_pub* p) {
    delete p;  // refptrs in the struct release on destruction.
}

// --- Audio ---

}  // extern "C" (close + re-open around the struct definition)

struct cppshim_audio_pub {
    agora::agora_refptr<ar::IAudioEncodedFrameSender> sender;
    agora::agora_refptr<ar::ILocalAudioTrack> track;
};

extern "C" {

cppshim_audio_pub* cppshim_audio_encoded_create(
    void* c_service_handle,
    void* c_factory_handle) {
    auto* service = deref_c_handle<ab::IAgoraService>(c_service_handle);
    auto* factory = deref_c_handle<ar::IMediaNodeFactory>(c_factory_handle);
    if (!service || !factory) return nullptr;

    auto sender = factory->createAudioEncodedFrameSender();
    if (!sender) return nullptr;

    // TMixMode::MIX_ENABLED (0) — matches the previous flat-C call where we
    // passed mix_mode=0. Either value works for a single-source track.
    auto track = service->createCustomAudioTrack(sender, agora::base::MIX_ENABLED);
    if (!track) return nullptr;
    track->setEnabled(true);

    return new cppshim_audio_pub{sender, track};
}

int cppshim_audio_encoded_send_aac(
    cppshim_audio_pub* p,
    const uint8_t* buf,
    uint32_t len,
    int sample_rate,
    int samples_per_channel,
    int channels) {
    if (!p || !p->sender || !buf || len == 0) return -1;
    // Match the SDK sample's sample_send_aac fields exactly. Do not touch
    // advancedSettings (defaults: speech=true, sendEvenIfEmpty=true).
    ar::EncodedAudioFrameInfo info;
    info.codec = ar::AUDIO_CODEC_AACLC;
    info.sampleRateHz = sample_rate;
    info.samplesPerChannel = samples_per_channel;
    info.numberOfChannels = channels;
    bool ok = p->sender->sendEncodedAudioFrame(buf, len, info);
    return ok ? 0 : 1;
}

int cppshim_audio_encoded_publish(cppshim_audio_pub* p, void* c_conn_handle) {
    if (!p || !p->track) return -1;
    auto* conn = deref_c_handle<ar::IRtcConnection>(c_conn_handle);
    if (!conn) return -1;
    auto* local = conn->getLocalUser();
    if (!local) return -1;
    return local->publishAudio(p->track);
}

int cppshim_audio_encoded_unpublish(cppshim_audio_pub* p, void* c_conn_handle) {
    if (!p || !p->track) return -1;
    auto* conn = deref_c_handle<ar::IRtcConnection>(c_conn_handle);
    if (!conn) return -1;
    auto* local = conn->getLocalUser();
    if (!local) return -1;
    return local->unpublishAudio(p->track);
}

void cppshim_audio_encoded_destroy(cppshim_audio_pub* p) {
    delete p;
}

}  // extern "C"
