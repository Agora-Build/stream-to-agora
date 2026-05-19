// C++ implementation of the encoded-sender shim. See agora_shim.h.
//
// Why a C++ shim instead of the flat C API:
//
//  1. The flat C `agora_video_encoded_image_sender_send` /
//     `agora_audio_encoded_frame_sender_send` accept the first call then
//     reject every subsequent one (rc=1). The C++ virtuals
//     `IVideoEncodedImageSender::sendEncodedVideoImage` /
//     `IAudioEncodedFrameSender::sendEncodedAudioFrame` work correctly.
//
//  2. The SDK only wires the video media pipeline when an
//     ILocalUserObserver is registered BEFORE connect(). The flat C
//     observer path doesn't trigger this; the C++ observer does.
//
// The shim links the SDK's C++ headers and dispatches through the C++
// vtable. It recovers C++ object pointers from the flat-C handles Rust
// still holds (service / connection / factory) by dereferencing them —
// see deref_c_handle() below.
//
// Bitstream handling: the shim forwards each access unit / frame
// exactly as produced by the upstream parser (parse::h264 / parse::hevc
// / parse::ivf) — no SEI strip, no captureTimeMs override — byte-for-
// byte matching the corresponding SDK sample (sample_send_h264_pcm /
// sample_send_h265 / sample_send_ivfvp8). Per-codec setup, selected by
// the `codec_type` the Rust caller passes (video::video_codec_type):
//   - H.264 (codec_type 0): default SenderOptions, no encoder config —
//     the proven v0.2.2 path.
//   - H.265/VP8/VP9/AV1 (non-zero): setVideoEncoderConfiguration with
//     the matching codecType + per-frame info.codecType; VP8/VP9/AV1
//     also need per-frame info.width/height (no in-band SPS to derive
//     size from). SenderOptions.codecType is never set (matches every
//     SDK sample). A wrong/zero codec_type makes the SDK mislabel the
//     bitstream → subscribers get no decodable video.

#include "agora_shim.h"

#include "IAgoraService.h"
#include "NGIAgoraRtcConnection.h"
#include "NGIAgoraLocalUser.h"
#include "NGIAgoraMediaNodeFactory.h"
#include "NGIAgoraMediaNode.h"
#include "NGIAgoraVideoTrack.h"
#include "NGIAgoraAudioTrack.h"
#include "AgoraBase.h"

#include <cstdio>
#include <cstdlib>
#include <atomic>

namespace ar = agora::rtc;
namespace ab = agora::base;

// Env-gated diagnostic (STA_TRACE=1): localises where the encoded-video
// path breaks for a codec — the codec/dimensions actually handed to the
// SDK, whether it accepts frames, whether a subscriber negotiates the
// track (intra requests). Retained: it is what pinned the VP8/VP9/AV1
// codec-mislabel bug (see README §Known Issues); off unless STA_TRACE set.
static bool sta_trace() {
    static bool t = std::getenv("STA_TRACE") != nullptr;
    return t;
}

// No-op LocalUserObserver. The SDK sample registers one; without it, the
// SDK's RTP packetizer doesn't produce frames the WebRTC depacketizer can
// reassemble (browser sees packets arriving but framesReceived stays 0).
// We don't actually need the callbacks — just registering an observer is
// what flips the SDK into the working code path.
class NoopLocalUserObserver : public ar::ILocalUserObserver {
public:
    void onAudioTrackPublishStart(agora::agora_refptr<ar::ILocalAudioTrack>) override {}
    void onAudioTrackPublishSuccess(agora::agora_refptr<ar::ILocalAudioTrack>) override {}
    void onAudioTrackUnpublished(agora::agora_refptr<ar::ILocalAudioTrack>) override {}
    void onAudioTrackPublicationFailure(agora::agora_refptr<ar::ILocalAudioTrack>, agora::ERROR_CODE_TYPE) override {}
    void onLocalAudioTrackStatistics(const ar::LocalAudioStats&) override {}
    void onRemoteAudioTrackStatistics(agora::agora_refptr<ar::IRemoteAudioTrack>, const ar::RemoteAudioTrackStats&) override {}
    void onUserAudioTrackSubscribed(agora::user_id_t, agora::agora_refptr<ar::IRemoteAudioTrack>) override {}
    void onUserAudioTrackStateChanged(agora::user_id_t, agora::agora_refptr<ar::IRemoteAudioTrack>, ar::REMOTE_AUDIO_STATE, ar::REMOTE_AUDIO_STATE_REASON, int) override {}
    void onVideoTrackPublishStart(agora::agora_refptr<ar::ILocalVideoTrack>) override {}
    void onVideoTrackPublishSuccess(agora::agora_refptr<ar::ILocalVideoTrack>) override {}
    void onVideoTrackPublicationFailure(agora::agora_refptr<ar::ILocalVideoTrack>, agora::ERROR_CODE_TYPE) override {}
    void onVideoTrackUnpublished(agora::agora_refptr<ar::ILocalVideoTrack>) override {}
    void onLocalVideoTrackStatistics(agora::agora_refptr<ar::ILocalVideoTrack>, const ar::LocalVideoTrackStats&) override {}
    void onUserVideoTrackSubscribed(agora::user_id_t, const ar::VideoTrackInfo&, agora::agora_refptr<ar::IRemoteVideoTrack>) override {}
    void onUserVideoTrackStateChanged(agora::user_id_t, agora::agora_refptr<ar::IRemoteVideoTrack>, ar::REMOTE_VIDEO_STATE, ar::REMOTE_VIDEO_STATE_REASON, int) override {}
    void onFirstRemoteVideoFrameRendered(agora::user_id_t, int, int, int) override {}
    void onRemoteVideoTrackStatistics(agora::agora_refptr<ar::IRemoteVideoTrack>, const ar::RemoteVideoTrackStats&) override {}
    void onAudioVolumeIndication(const ar::AudioVolumeInformation*, unsigned int, int) override {}
    void onActiveSpeaker(agora::user_id_t) override {}
    void onAudioSubscribeStateChanged(const char*, agora::user_id_t, ar::STREAM_SUBSCRIBE_STATE, ar::STREAM_SUBSCRIBE_STATE, int) override {}
    void onVideoSubscribeStateChanged(const char*, agora::user_id_t, ar::STREAM_SUBSCRIBE_STATE, ar::STREAM_SUBSCRIBE_STATE, int) override {}
    void onAudioPublishStateChanged(const char*, ar::STREAM_PUBLISH_STATE, ar::STREAM_PUBLISH_STATE, int) override {}
    void onVideoPublishStateChanged(const char*, ar::STREAM_PUBLISH_STATE, ar::STREAM_PUBLISH_STATE, int) override {}
    void onFirstRemoteAudioFrame(agora::user_id_t, int) override {}
    void onFirstRemoteAudioDecoded(agora::user_id_t, int) override {}
    void onFirstRemoteVideoFrame(agora::user_id_t, int, int, int) override {}
    void onFirstRemoteVideoDecoded(agora::user_id_t, int, int, int) override {}
    void onVideoSizeChanged(agora::user_id_t, int, int, int) override {}

    // Subscriber-driven keyframe request (PLI). A future enhancement
    // could signal the pump to emit a fresh IDR on demand.
    void onIntraRequestReceived() override {
        if (sta_trace()) std::fprintf(stderr, "shim.intra\n");
    }
    void onLocalVideoTrackStateChanged(agora::agora_refptr<ar::ILocalVideoTrack>,
                                       ar::LOCAL_VIDEO_STREAM_STATE,
                                       ar::LOCAL_VIDEO_STREAM_REASON) override {}
};

// No-op C++ IRtcConnectionObserver. Registering one (vs only the flat-C
// observer Rust uses to learn "connected") is what wires the video RTCP
// feedback path. Without it, the SDK accepts every encoded video frame
// (rc=0) but the RTP it emits never assembles into a frame at WebRTC
// subscribers — audio survives, video stays black. The flat-C observer
// keeps feeding Rust the Connected event; this one exists purely to flip
// the SDK into the working code path, exactly like NoopLocalUserObserver.
class NoopConnObserver : public ar::IRtcConnectionObserver {
public:
    void onConnected(const ar::TConnectionInfo&, ar::CONNECTION_CHANGED_REASON_TYPE) override {}
    void onDisconnected(const ar::TConnectionInfo&, ar::CONNECTION_CHANGED_REASON_TYPE) override {}
    void onConnecting(const ar::TConnectionInfo&, ar::CONNECTION_CHANGED_REASON_TYPE) override {}
    void onReconnecting(const ar::TConnectionInfo&, ar::CONNECTION_CHANGED_REASON_TYPE) override {}
    void onReconnected(const ar::TConnectionInfo&, ar::CONNECTION_CHANGED_REASON_TYPE) override {}
    void onConnectionLost(const ar::TConnectionInfo&) override {}
    void onLastmileQuality(const ar::QUALITY_TYPE) override {}
    void onLastmileProbeResult(const ar::LastmileProbeResult&) override {}
    void onTokenPrivilegeWillExpire(const char*) override {}
    void onTokenPrivilegeDidExpire() override {}
    void onConnectionFailure(const ar::TConnectionInfo&, ar::CONNECTION_CHANGED_REASON_TYPE) override {}
    void onUserJoined(agora::user_id_t) override {}
    void onUserLeft(agora::user_id_t, ar::USER_OFFLINE_REASON_TYPE) override {}
    void onTransportStats(const ar::RtcStats&) override {}
    void onChannelMediaRelayStateChanged(int, int) override {}
};

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

    // Match every SDK sample (sample_send_h264_pcm / h265 / ivfvp8):
    // they set ONLY ccMode=CC_ENABLED on SenderOptions and never touch
    // SenderOptions.codecType — routing is done by the per-frame
    // EncodedVideoFrameInfo.codecType + setVideoEncoderConfiguration.
    // (Setting opts.codecType is a no-op for H.264/H.265 — it equals the
    // C++ default — but for VP8/VP9/AV1 it diverges from the sample and
    // stops the track from being published.) CC_ENABLED is also the
    // default, so this is byte-identical for the proven H.26x paths.
    ar::SenderOptions opts;
    opts.ccMode = ar::TCcMode::CC_ENABLED;

    auto track = service->createCustomVideoTrack(sender, opts);
    if (!track) return nullptr;

    // Every non-H.264 codec must pin the track's encoder config to its
    // codec, or the SDK never negotiates/packetizes it and subscribers
    // get no video. The SDK's own samples confirm this: sample_send_h265
    // and sample_send_ivfvp8 both call setVideoEncoderConfiguration with
    // the matching codecType; sample_send_h264_pcm does NOT (H.264 is the
    // default — codec_type 0 here keeps that proven path untouched).
    if (codec_type != 0) {
        ar::VideoEncoderConfiguration ec;
        ec.codecType = static_cast<ar::VIDEO_CODEC_TYPE>(codec_type);
        track->setVideoEncoderConfiguration(ec);
    }

    return new cppshim_video_pub{sender, track};
}

int cppshim_video_encoded_send(
    cppshim_video_pub* p,
    const uint8_t* buf,
    uint32_t len,
    int is_keyframe,
    int fps,
    int codec_type,
    int width,
    int height,
    int64_t capture_time_ms) {
    if (!p || !p->sender || !buf || len == 0) return -1;

    // Send the AU exactly as the upstream splitter produced it (leading
    // VPS/SPS/PPS/SEI included) — byte-for-byte what the SDK samples'
    // sendOneH26xFrame do. Stripping SEI or setting captureTimeMs are
    // both deviations from the sample that broke subscriber decode; the
    // splitter (parse::h264 / parse::hevc) already groups the parameter
    // sets into the keyframe AU, and framesPerSecond alone drives the
    // SDK timestamping. `codec_type` (0 → H264) selects the per-frame
    // codec, which is what actually routes the bitstream.
    (void)capture_time_ms;

    // Per-frame info, byte-identical to the matching SDK sample:
    //  - H.264/H.265 (codec_type 0/3): sample_send_h264_pcm / h265 set
    //    {rotation, codecType, framesPerSecond, frameType} and derive
    //    size from the in-band SPS — leave this proven path untouched.
    //  - VP8/VP9/AV1: sample_send_ivfvp8 sets {rotation, codecType,
    //    frameType, width, height} and does NOT set framesPerSecond.
    //    VP8/VP9/AV1 carry no SPS, so without explicit width/height the
    //    encoded track stays 0x0 and is never published (no remote user).
    const bool is_h26x = (codec_type == 0 || codec_type == 3);
    ar::EncodedVideoFrameInfo info;
    info.rotation = ar::VIDEO_ORIENTATION_0;
    info.codecType = codec_type
        ? static_cast<ar::VIDEO_CODEC_TYPE>(codec_type)
        : ar::VIDEO_CODEC_H264;
    info.frameType = is_keyframe
        ? ar::VIDEO_FRAME_TYPE_KEY_FRAME
        : ar::VIDEO_FRAME_TYPE_DELTA_FRAME;
    if (is_h26x) {
        info.framesPerSecond = fps;
    } else if (width > 0 && height > 0) {
        info.width = width;
        info.height = height;
    }
    bool ok = p->sender->sendEncodedVideoImage(buf, len, info);
    if (sta_trace()) {
        static std::atomic<uint64_t> n{0};
        uint64_t i = n.fetch_add(1);
        if (i < 6 || i % 120 == 0) {
            std::fprintf(stderr,
                "shim.vid[%llu] codec=%d len=%u key=%d %dx%d rc=%s\n",
                (unsigned long long)i, codec_type, len, is_keyframe,
                info.width, info.height, ok ? "ok" : "FAIL");
        }
    }
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
    void* c_factory_handle,
    int codec) {
    auto* service = deref_c_handle<ab::IAgoraService>(c_service_handle);
    auto* factory = deref_c_handle<ar::IMediaNodeFactory>(c_factory_handle);
    if (!service || !factory) return nullptr;

    auto sender = factory->createAudioEncodedFrameSender();
    if (!sender) return nullptr;

    // Mix mode follows the SDK samples: AAC (sample_send_aac) uses
    // MIX_ENABLED — the proven v0.2.2 path; Opus (sample_send_opus) uses
    // MIX_DISABLED.
    auto mix = (codec == 8) ? agora::base::MIX_ENABLED
                            : agora::base::MIX_DISABLED;
    auto track = service->createCustomAudioTrack(sender, mix);
    if (!track) return nullptr;

    return new cppshim_audio_pub{sender, track};
}

int cppshim_audio_encoded_send(
    cppshim_audio_pub* p,
    const uint8_t* buf,
    uint32_t len,
    int codec,
    int sample_rate,
    int samples_per_channel,
    int channels) {
    if (!p || !p->sender || !buf || len == 0) return -1;

    // Match the SDK samples (sample_send_aac / sample_send_opus) exactly.
    // Do not touch advancedSettings (defaults: speech=true,
    // sendEvenIfEmpty=true).
    ar::EncodedAudioFrameInfo info;
    info.codec = static_cast<ar::AUDIO_CODEC_TYPE>(codec);
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

// --- LocalUserObserver registration ---

struct cppshim_local_user_observer {
    NoopLocalUserObserver impl;
    ar::ILocalUser* local;  // borrowed; we don't own it (conn does)
};

cppshim_local_user_observer* cppshim_local_user_observer_register(void* c_conn_handle) {
    auto* conn = deref_c_handle<ar::IRtcConnection>(c_conn_handle);
    if (!conn) return nullptr;
    auto* local = conn->getLocalUser();
    if (!local) return nullptr;
    auto* obs = new cppshim_local_user_observer{};
    obs->local = local;
    if (local->registerLocalUserObserver(&obs->impl) != 0) {
        delete obs;
        return nullptr;
    }
    return obs;
}

void cppshim_local_user_observer_destroy(cppshim_local_user_observer* obs) {
    if (!obs) return;
    if (obs->local) obs->local->unregisterLocalUserObserver(&obs->impl);
    delete obs;
}

// --- ConnectionObserver registration ---

struct cppshim_conn_observer {
    NoopConnObserver impl;
    ar::IRtcConnection* conn;  // borrowed
};

cppshim_conn_observer* cppshim_conn_observer_register(void* c_conn_handle) {
    auto* conn = deref_c_handle<ar::IRtcConnection>(c_conn_handle);
    if (!conn) return nullptr;
    auto* obs = new cppshim_conn_observer{};
    obs->conn = conn;
    if (conn->registerObserver(&obs->impl) != 0) {
        delete obs;
        return nullptr;
    }
    return obs;
}

void cppshim_conn_observer_destroy(cppshim_conn_observer* obs) {
    if (!obs) return;
    if (obs->conn) obs->conn->unregisterObserver(&obs->impl);
    delete obs;
}

}  // extern "C"
