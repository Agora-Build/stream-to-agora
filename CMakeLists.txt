# stream-to-agora — native C++ shim build.
#
# Builds `libagora_shim.a`, a thin C-ABI wrapper around the Agora
# RTSA SDK that the Rust binary links to via FFI.
#
# The Agora RTSA SDK is downloaded from agora.io at configure time
# (per platform) into `${CMAKE_BINARY_DIR}/agora-sdk/`. To use a
# pre-staged SDK instead, set `-DAGORA_RTC_SDK_PATH=/path/to/sdk`
# and we'll skip the download.

cmake_minimum_required(VERSION 3.16)
project(stream_to_agora_native LANGUAGES C CXX)

set(CMAKE_C_STANDARD 11)
set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_POSITION_INDEPENDENT_CODE ON)

# ── Platform → SDK URL ────────────────────────────────────────────────
# Agora ships per-OS/arch tarballs. Add new platforms here as we get
# the URLs. Each tarball extracts into a top-level dir like
# `Agora_Native_SDK_for_All_Platforms/agora_sdk/` containing
# `include/` (C headers) and a lib dir.

set(AGORA_RTC_SDK_VERSION "v4.4.32-20250425_144419-675648")

if(NOT DEFINED AGORA_SDK_URL)
    if(CMAKE_SYSTEM_NAME STREQUAL "Linux" AND CMAKE_SYSTEM_PROCESSOR MATCHES "^(x86_64|amd64)$")
        set(AGORA_SDK_URL "https://download.agora.io/rtsasdk/release/Agora-RTC-x86_64-linux-gnu-${AGORA_RTC_SDK_VERSION}.tgz")
        set(AGORA_SDK_DIRNAME "agora_rtc_sdk")
    elseif(CMAKE_SYSTEM_NAME STREQUAL "Linux" AND CMAKE_SYSTEM_PROCESSOR MATCHES "^(aarch64|arm64)$")
        # TODO: drop in the Linux aarch64 RTSA URL when we have it.
        message(FATAL_ERROR "Linux aarch64 SDK URL not configured yet — set -DAGORA_SDK_URL=...")
    elseif(CMAKE_SYSTEM_NAME STREQUAL "Darwin" AND CMAKE_SYSTEM_PROCESSOR MATCHES "^(x86_64)$")
        # TODO: confirm exact filename.
        set(AGORA_SDK_URL "https://download.agora.io/rtsasdk/release/Agora-RTC-x86_64-apple-darwin-${AGORA_RTC_SDK_VERSION}.tgz")
        set(AGORA_SDK_DIRNAME "agora_rtc_sdk")
    elseif(CMAKE_SYSTEM_NAME STREQUAL "Darwin" AND CMAKE_SYSTEM_PROCESSOR MATCHES "^(arm64|aarch64)$")
        set(AGORA_SDK_URL "https://download.agora.io/rtsasdk/release/Agora-RTC-arm64-apple-darwin-${AGORA_RTC_SDK_VERSION}.tgz")
        set(AGORA_SDK_DIRNAME "agora_rtc_sdk")
    else()
        message(FATAL_ERROR "Unsupported platform: ${CMAKE_SYSTEM_NAME}/${CMAKE_SYSTEM_PROCESSOR}")
    endif()
endif()

# ── Stage the SDK ─────────────────────────────────────────────────────
# Two paths: caller passed -DAGORA_RTC_SDK_PATH=… (use it as-is) or
# we fetch the tarball into the build dir and unpack once.

if(DEFINED AGORA_RTC_SDK_PATH AND NOT AGORA_RTC_SDK_PATH STREQUAL "")
    message(STATUS "Using pre-staged Agora RTSA SDK at ${AGORA_RTC_SDK_PATH}")
    set(AGORA_SDK_ROOT "${AGORA_RTC_SDK_PATH}")
else()
    set(AGORA_SDK_STAGE  "${CMAKE_BINARY_DIR}/agora-sdk")
    set(AGORA_SDK_TARBALL "${AGORA_SDK_STAGE}/sdk.tgz")
    set(AGORA_SDK_ROOT    "${AGORA_SDK_STAGE}/${AGORA_SDK_DIRNAME}")

    if(NOT EXISTS "${AGORA_SDK_ROOT}")
        file(MAKE_DIRECTORY "${AGORA_SDK_STAGE}")
        message(STATUS "Downloading Agora RTSA SDK from ${AGORA_SDK_URL}")
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
# RTSA tarballs vary slightly across versions but the top-level layout
# is reliably `agora_sdk/{include,low_level_api/include,libs,...}`.
# We probe both common roots to stay version-agnostic.

find_path(AGORA_RTC_INCLUDE_DIR
    NAMES IAgoraService.h NGIAgoraRtcConnection.h
    PATHS
        "${AGORA_SDK_ROOT}/agora_sdk/include"
        "${AGORA_SDK_ROOT}/include"
    NO_DEFAULT_PATH
)
if(NOT AGORA_RTC_INCLUDE_DIR)
    message(FATAL_ERROR
        "Couldn't find Agora RTSA headers under ${AGORA_SDK_ROOT}. "
        "Inspect the extracted tarball and update CMakeLists.txt's find_path() PATHS.")
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
        "Inspect the extracted tarball and update CMakeLists.txt's find_library() PATHS.")
endif()

message(STATUS "Agora RTSA include: ${AGORA_RTC_INCLUDE_DIR}")
message(STATUS "Agora RTSA library: ${AGORA_RTC_LIB}")

# ── The shim ─────────────────────────────────────────────────────────
add_library(agora_shim STATIC
    native/src/agora_shim.cpp
)
target_include_directories(agora_shim
    PUBLIC  native/include
    PRIVATE "${AGORA_RTC_INCLUDE_DIR}"
)
target_compile_options(agora_shim PRIVATE -Wall -Wextra -fvisibility=hidden)

# Expose where the SDK runtime lib lives so build.rs can link & set
# rpath. Written as a small text file Cargo's build.rs can read.
get_filename_component(AGORA_RTC_LIB_DIR "${AGORA_RTC_LIB}" DIRECTORY)
file(WRITE "${CMAKE_BINARY_DIR}/agora_sdk_paths.txt"
    "AGORA_RTC_LIB_DIR=${AGORA_RTC_LIB_DIR}\n"
    "AGORA_RTC_LIB=${AGORA_RTC_LIB}\n"
    "AGORA_RTC_INCLUDE_DIR=${AGORA_RTC_INCLUDE_DIR}\n"
)

# Let consumers (cargo `cmake` crate) see where the static lib lands.
install(TARGETS agora_shim ARCHIVE DESTINATION lib)
install(FILES "${CMAKE_BINARY_DIR}/agora_sdk_paths.txt" DESTINATION .)
