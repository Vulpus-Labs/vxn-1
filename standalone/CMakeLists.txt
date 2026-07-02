# Standalone app wrapper (E014 / tickets 0027-0031).
#
# Drives free-audio/clap-wrapper in bundled single-binary mode: the wrapper's
# standalone glue (RtAudio output, RtMidi input, native main window) is compiled
# here and the prebuilt static archive is force-loaded in so the CLAP entry point
# and the whole engine live inside the standalone binary — no external `.clap`.
#
# This CMake is NOT invoked directly; `cargo xtask standalone [--vxn1|--vxn2]`
# (tickets 0028-0029) builds the Rust staticlib, then configures and builds this
# project, passing the cache variables below.
#
# Manual invocation (from repo root):
#   cmake -S standalone -B target/standalone-test \
#       -DVXN_CLAP_STATIC=$PWD/target/release/libvxn_clap.a \
#       -DVXN_CLAP_SDK_DIR=$PWD/vendor/clap \
#       -DVXN_CLAP_WRAPPER_DIR=$PWD/vendor/clap-wrapper \
#       -DVXN_OUTPUT_DIR=$PWD/target/standalone-test/out \
#       -DVXN_PLUGIN_NAME=VXN1 \
#       -DVXN_BUNDLE_ID=labs.vulpus.vxn1.standalone
#   cmake --build target/standalone-test

cmake_minimum_required(VERSION 3.21)

if(APPLE AND NOT DEFINED CMAKE_OSX_DEPLOYMENT_TARGET)
  set(CMAKE_OSX_DEPLOYMENT_TARGET "11.0" CACHE STRING "Minimum macOS version")
endif()

project(vxn-standalone LANGUAGES C CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

if(APPLE)
  enable_language(OBJC)
  enable_language(OBJCXX)
endif()

# ---------------------------------------------------------------------------
# Inputs (passed by xtask). All required except VXN_OUTPUT_DIR.
# ---------------------------------------------------------------------------
set(VXN_CLAP_STATIC      "" CACHE STRING "Path(s) to libvxn_clap.a / vxn_clap.lib or libvxn2_clap.a")
set(VXN_CLAP_SDK_DIR     "" CACHE PATH   "Path to vendor/clap checkout")
set(VXN_CLAP_WRAPPER_DIR "" CACHE PATH   "Path to vendor/clap-wrapper checkout")
set(VXN_OUTPUT_DIR "${CMAKE_BINARY_DIR}/out" CACHE PATH "Directory for the finished standalone binary / .app")
set(VXN_PLUGIN_NAME      "VXN1" CACHE STRING "Output binary name (VXN1 or VXN2)")
set(VXN_BUNDLE_ID        "labs.vulpus.vxn1.standalone" CACHE STRING "macOS bundle identifier")

foreach(_req VXN_CLAP_STATIC VXN_CLAP_SDK_DIR VXN_CLAP_WRAPPER_DIR)
  if("${${_req}}" STREQUAL "")
    message(FATAL_ERROR "vxn-standalone: ${_req} must be set (see CMakeLists.txt header).")
  endif()
endforeach()

# ---------------------------------------------------------------------------
# clap-wrapper configuration: bundled, standalone only.
# ---------------------------------------------------------------------------
# The standalone wrapper needs RtAudio/RtMidi. clap-wrapper 0.12.1 does not
# vendor them as submodules (its libs/ holds only fmt + psl), so a recursive
# submodule checkout cannot supply them and `search_for_sdk_source` finds
# nothing on a clean tree (e.g. CI). Let clap-wrapper CPM-fetch them at configure
# time — pinned tags (rtaudio 6.0.1, rtmidi) keep it reproducible. The CLAP SDK
# still comes from our vendored checkout via CLAP_SDK_ROOT.
set(CLAP_SDK_ROOT              "${VXN_CLAP_SDK_DIR}" CACHE STRING "" FORCE)
set(CLAP_WRAPPER_DOWNLOAD_DEPENDENCIES TRUE  CACHE BOOL "" FORCE)
set(CLAP_WRAPPER_BUILD_AUV2            FALSE CACHE BOOL "" FORCE)
set(CLAP_WRAPPER_BUILD_VST3            FALSE CACHE BOOL "" FORCE)
set(CLAP_WRAPPER_BUILD_STANDALONE      TRUE  CACHE BOOL "" FORCE)
set(CLAP_WRAPPER_BUILD_TESTS           FALSE CACHE BOOL "" FORCE)
set(CLAP_WRAPPER_OUTPUT_NAME "${VXN_PLUGIN_NAME}" CACHE STRING "" FORCE)

add_subdirectory("${VXN_CLAP_WRAPPER_DIR}" clap-wrapper-build EXCLUDE_FROM_ALL)

# ---------------------------------------------------------------------------
# Standalone executable: clap-wrapper standalone glue + our static CLAP.
# ---------------------------------------------------------------------------
add_executable(vxn_standalone)
set_target_properties(vxn_standalone PROPERTIES OUTPUT_NAME "${VXN_PLUGIN_NAME}")

# STATICALLY_LINKED_CLAP_ENTRY: we whole-archive the CLAP below, so clap_entry
# is linked into the binary. Without this the wrapper compiles its dynamic-load
# path, which references HOSTED_CLAP_NAME (undefined here) and fails to build.
target_add_standalone_wrapper(
  TARGET vxn_standalone
  OUTPUT_NAME "${VXN_PLUGIN_NAME}"
  BUNDLE_IDENTIFIER "${VXN_BUNDLE_ID}"
  BUNDLE_VERSION "1.0"
  STATICALLY_LINKED_CLAP_ENTRY TRUE
)

# Force-load the static archive so clap_entry and the whole engine are pulled
# in — without whole-archive the linker would strip unreferenced objects.
foreach(_archive ${VXN_CLAP_STATIC})
  if(APPLE)
    target_link_options(vxn_standalone PRIVATE "-Wl,-force_load,${_archive}")
  elseif(WIN32)
    target_link_options(vxn_standalone PRIVATE "/WHOLEARCHIVE:${_archive}")
  else()
    target_link_options(vxn_standalone PRIVATE
      "-Wl,--whole-archive" "${_archive}" "-Wl,--no-whole-archive")
  endif()
endforeach()

# Transitive native deps that a Rust staticlib doesn't record. Keep in sync
# with `otool -L libvxn_clap.dylib` on the working CLAP build.
if(APPLE)
  target_link_libraries(vxn_standalone PRIVATE
    "-framework AppKit"
    "-framework WebKit"
    "-framework QuartzCore"
    "-framework CoreGraphics"
    "-framework Foundation"
    "-framework CoreFoundation"
    "-liconv")
elseif(WIN32)
  target_link_libraries(vxn_standalone PRIVATE
    ws2_32 userenv ntdll bcrypt
    user32 gdi32 shell32 ole32 oleaut32 advapi32 version
    windowsapp)  # WebView2 requires WindowsApp
elseif(UNIX)
  target_link_libraries(vxn_standalone PRIVATE dl pthread m)
endif()

# ---------------------------------------------------------------------------
# Stage the finished binary / .app into VXN_OUTPUT_DIR.
# ---------------------------------------------------------------------------
add_custom_command(TARGET vxn_standalone POST_BUILD
  COMMAND ${CMAKE_COMMAND} -E make_directory "${VXN_OUTPUT_DIR}")

if(APPLE)
  add_custom_command(TARGET vxn_standalone POST_BUILD
    COMMAND ${CMAKE_COMMAND} -E rm -rf "${VXN_OUTPUT_DIR}/${VXN_PLUGIN_NAME}.app"
    COMMAND ${CMAKE_COMMAND} -E copy_directory
            "$<TARGET_BUNDLE_DIR:vxn_standalone>" "${VXN_OUTPUT_DIR}/${VXN_PLUGIN_NAME}.app"
    COMMENT "vxn-standalone: staging ${VXN_PLUGIN_NAME}.app → ${VXN_OUTPUT_DIR}")
else()
  add_custom_command(TARGET vxn_standalone POST_BUILD
    COMMAND ${CMAKE_COMMAND} -E copy_if_different
            "$<TARGET_FILE:vxn_standalone>" "${VXN_OUTPUT_DIR}/${VXN_PLUGIN_NAME}.exe"
    COMMENT "vxn-standalone: staging ${VXN_PLUGIN_NAME}.exe → ${VXN_OUTPUT_DIR}")
endif()
