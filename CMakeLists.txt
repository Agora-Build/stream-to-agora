# stream-to-agora — Agora SDK staging.
#
# Job: download (or use a pre-staged) Agora NG/RTSA SDK tarball,
# locate its C headers + shared library, and emit
# `agora_sdk_paths.txt` so Cargo's build.rs can wire `cargo:rustc-link-*`
# directives.
#
# We don't compile any C++ code here — the SDK exposes a flat C API
# under `include/c/api2/`, which Rust calls directly via `extern "C"`.
#
# Override the auto-download with `-DAGORA_RTC_SDK_PATH=/path/to/sdk`
# pointing at a pre-extracted SDK tree (parent of `agora_sdk/`).

cmake_minimum_required(VERSION 3.16)
project(stream_to_agora_native NONE)

set(AGORA_RTC_SDK_VERSION "v4.4.32-20250425_144419-675648")

# ── Platform → SDK URL ────────────────────────────────────────────────
if(NOT DEFINED AGORA_SDK_URL)
    if(CMAKE_SYSTEM_NAME STREQUAL "Linux" AND CMAKE_SYSTEM_PROCESSOR MATCHES "^(x86_64|amd64)$")
        set(AGORA_SDK_URL "https://download.agora.io/rtsasdk/release/Agora-RTC-x86_64-linux-gnu-${AGORA_RTC_SDK_VERSION}.tgz")
    elseif(CMAKE_SYSTEM_NAME STREQUAL "Linux" AND CMAKE_SYSTEM_PROCESSOR MATCHES "^(aarch64|arm64)$")
        # TODO: confirm the exact aarch64 URL from Agora console.
        message(FATAL_ERROR "Linux aarch64 SDK URL not configured yet — pass -DAGORA_SDK_URL=...")
    elseif(CMAKE_SYSTEM_NAME STREQUAL "Darwin" AND CMAKE_SYSTEM_PROCESSOR MATCHES "^(x86_64)$")
        set(AGORA_SDK_URL "https://download.agora.io/rtsasdk/release/Agora-RTC-x86_64-apple-darwin-${AGORA_RTC_SDK_VERSION}.tgz")
    elseif(CMAKE_SYSTEM_NAME STREQUAL "Darwin" AND CMAKE_SYSTEM_PROCESSOR MATCHES "^(arm64|aarch64)$")
        set(AGORA_SDK_URL "https://download.agora.io/rtsasdk/release/Agora-RTC-arm64-apple-darwin-${AGORA_RTC_SDK_VERSION}.tgz")
    else()
        message(FATAL_ERROR "Unsupported platform: ${CMAKE_SYSTEM_NAME}/${CMAKE_SYSTEM_PROCESSOR}")
    endif()
endif()

# Tarballs extract to a top-level directory named `agora_rtc_sdk`.
set(AGORA_SDK_DIRNAME "agora_rtc_sdk")

# ── Stage the SDK ─────────────────────────────────────────────────────
if(DEFINED AGORA_RTC_SDK_PATH AND NOT AGORA_RTC_SDK_PATH STREQUAL "")
    message(STATUS "Using pre-staged Agora SDK at ${AGORA_RTC_SDK_PATH}")
    set(AGORA_SDK_ROOT "${AGORA_RTC_SDK_PATH}")
else()
    set(AGORA_SDK_STAGE   "${CMAKE_BINARY_DIR}/agora-sdk")
    set(AGORA_SDK_TARBALL "${AGORA_SDK_STAGE}/sdk.tgz")
    set(AGORA_SDK_ROOT    "${AGORA_SDK_STAGE}/${AGORA_SDK_DIRNAME}")

    if(NOT EXISTS "${AGORA_SDK_ROOT}")
        file(MAKE_DIRECTORY "${AGORA_SDK_STAGE}")
        message(STATUS "Downloading Agora SDK from ${AGORA_SDK_URL}")
        file(DOWNLOAD
            "${AGORA_SDK_URL}"
            "${AGORA_SDK_TARBALL}"
            SHOW_PROGRESS
            STATUS    AGORA_DL_STATUS
            TLS_VERIFY ON
        )
        list(GET AGORA_DL_STATUS 0 AGORA_DL_CODE)
        if(NOT AGORA_DL_CODE EQUAL 0)
            list(GET AGORA_DL_STATUS 1 AGORA_DL_MSG)
            message(FATAL_ERROR "SDK download failed (${AGORA_DL_CODE}): ${AGORA_DL_MSG}")
        endif()
        message(STATUS "Extracting SDK into ${AGORA_SDK_STAGE}")
        execute_process(
            COMMAND ${CMAKE_COMMAND} -E tar xzf "${AGORA_SDK_TARBALL}"
            WORKING_DIRECTORY "${AGORA_SDK_STAGE}"
            RESULT_VARIABLE   AGORA_TAR_RC
        )
        if(NOT AGORA_TAR_RC EQUAL 0)
            message(FATAL_ERROR "SDK tarball extraction failed: rc=${AGORA_TAR_RC}")
        endif()
    endif()
endif()

# ── Locate include / lib inside the SDK tree ──────────────────────────
find_path(AGORA_RTC_INCLUDE_DIR
    NAMES IAgoraService.h NGIAgoraRtcConnection.h
    PATHS
        "${AGORA_SDK_ROOT}/agora_sdk/include"
        "${AGORA_SDK_ROOT}/include"
    NO_DEFAULT_PATH
)
if(NOT AGORA_RTC_INCLUDE_DIR)
    message(FATAL_ERROR
        "Couldn't find Agora SDK headers under ${AGORA_SDK_ROOT}. "
        "Inspect the extracted tarball and update the find_path() PATHS in CMakeLists.txt.")
endif()

find_library(AGORA_RTC_LIB
    NAMES agora_rtc_sdk agora-rtc-sdk
    PATHS
        "${AGORA_SDK_ROOT}/agora_sdk"
        "${AGORA_SDK_ROOT}/agora_sdk/lib"
        "${AGORA_SDK_ROOT}/lib"
    NO_DEFAULT_PATH
)
if(NOT AGORA_RTC_LIB)
    message(FATAL_ERROR
        "Couldn't find libagora_rtc_sdk under ${AGORA_SDK_ROOT}. "
        "Inspect the extracted tarball and update the find_library() PATHS in CMakeLists.txt.")
endif()

message(STATUS "Agora SDK include: ${AGORA_RTC_INCLUDE_DIR}")
message(STATUS "Agora SDK library: ${AGORA_RTC_LIB}")

# ── Emit paths sidecar for Cargo's build.rs ───────────────────────────
get_filename_component(AGORA_RTC_LIB_DIR "${AGORA_RTC_LIB}" DIRECTORY)
file(WRITE "${CMAKE_BINARY_DIR}/agora_sdk_paths.txt"
    "AGORA_RTC_LIB_DIR=${AGORA_RTC_LIB_DIR}\n"
    "AGORA_RTC_LIB=${AGORA_RTC_LIB}\n"
    "AGORA_RTC_INCLUDE_DIR=${AGORA_RTC_INCLUDE_DIR}\n"
)

install(FILES "${CMAKE_BINARY_DIR}/agora_sdk_paths.txt" DESTINATION .)
