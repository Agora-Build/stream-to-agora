// agora_shim.h — C ABI exported to Rust over FFI.
//
// Phase 0 (this commit): empty header; the file exists so the build
// script's rerun-if-changed declaration has a target.
//
// Phase 1 will add:
//   typedef struct AgoraEngine AgoraEngine;
//   AgoraEngine* sta_engine_create(const char* app_id);
//   int  sta_engine_join(AgoraEngine*, const char* channel, const char* token, const char* uid);
//   int  sta_engine_push_video_frame(AgoraEngine*, const uint8_t* yuv, int w, int h, int64_t pts_ms);
//   int  sta_engine_push_audio_frame(AgoraEngine*, const int16_t* pcm, int samples, int sample_rate, int channels);
//   void sta_engine_leave(AgoraEngine*);
//   void sta_engine_destroy(AgoraEngine*);

#ifndef STA_AGORA_SHIM_H
#define STA_AGORA_SHIM_H

#ifdef __cplusplus
extern "C" {
#endif

// (Phase 1 declarations land here.)

#ifdef __cplusplus
}
#endif

#endif  // STA_AGORA_SHIM_H
