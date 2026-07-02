# VXN1 VST3 wrapper (E010 / ADR 0008 §1, §2).
#
# Drives free-audio/clap-wrapper in bundled single-binary mode: the wrapper's
# VST3 glue is compiled here and the prebuilt `vxn-clap` static archive (ticket
# 0008) is force-loaded into the resulting MODULE so the CLAP entry point and
# the whole engine live inside `VXN1.vst3` — no external `.clap` at runtime.
#
# This CMake is NOT invoked directly by users; `cargo xtask bundle --format
# vst3` (tickets 0011/0012) builds the Rust staticlib, then configures and
# builds this project, passing the cache variables below. The CLAP-only build
# path never touches CMake.
#
# Standalone manual invocation (matches ticket 0010 acceptance), from repo root:
#   cmake -S vxn-1/wrapper -B target/wrapper-test \
#       -DVXN_CLAP_STATIC=$PWD/target/release/libvxn_clap.a \
#       -DVXN_CLAP_SDK_DIR=$PWD/vendor/clap \
#       -DVXN_VST3_SDK_DIR=$PWD/vendor/vst3sdk \
#       -DVXN_CLAP_WRAPPER_DIR=$PWD/vendor/clap-wrapper \
#       -DVXN_OUTPUT_DIR=$PWD/target/wrapper-test/out
#   cmake --build target/wrapper-test

cmake_minimum_required(VERSION 3.21)

# Must be set before the first project()/enable_language so the wrapper sees it:
# >= 10.15 makes clap-wrapper use std::filesystem instead of CPM-fetching the
# gulrak polyfill, which keeps the build fully offline (it otherwise defaults to
# 10.13). 11.0 matches the Rust aarch64-apple-darwin minimum.
if(APPLE AND NOT DEFINED CMAKE_OSX_DEPLOYMENT_TARGET)
  set(CMAKE_OSX_DEPLOYMENT_TARGET "11.0" CACHE STRING "Minimum macOS version")
endif()

project(vxn1-wrapper LANGUAGES C CXX)

# clap-wrapper and the VST3 SDK require C++17, but the wrapper only sets the
# standard when it is the top-level project. We drive it via add_subdirectory,
# so set it here; these propagate into the subdirectory targets.
set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

# The Rust staticlib links against the dynamic MSVC runtime (rustc's default,
# and what a `--release` build emits). Force every C++ target — the VST3 SDK
# and clap-wrapper glue included — onto the same dynamic runtime so the final
# link doesn't mix static (libcmt/libcpmt) and dynamic (msvcrt) CRTs, which
# LNK2005/LNK2038 out. CMP0091 NEW routes the choice through
# MSVC_RUNTIME_LIBRARY instead of hard-coded /MT|/MD compile flags.
if(WIN32)
  cmake_policy(SET CMP0091 NEW)
  set(CMAKE_MSVC_RUNTIME_LIBRARY "MultiThreaded$<$<CONFIG:Debug>:Debug>DLL")
endif()

# The wrapper compiles an Objective-C++ source (detail/os/macos.mm); enable the
# languages at top level so the toolchain is registered for the whole tree.
if(APPLE)
  enable_language(OBJC)
  enable_language(OBJCXX)
endif()

# ---------------------------------------------------------------------------
# Inputs (passed by xtask; see header). All required except VXN_OUTPUT_DIR.
# ---------------------------------------------------------------------------
# VXN_CLAP_STATIC may be a ';'-separated list. For a universal macOS build pass
# a SINGLE lipo'd fat archive — force_load of a thin wrong-arch archive fails
# the per-arch link slice, so two thin slices will not work here; xtask lipos
# the arm64+x86_64 slices into one fat archive before configuring (ticket 0011).
set(VXN_CLAP_STATIC      "" CACHE STRING "Path(s) to libvxn_clap.a / vxn_clap.lib")
set(VXN_CLAP_SDK_DIR     "" CACHE PATH   "Path to vendor/clap checkout")
set(VXN_VST3_SDK_DIR     "" CACHE PATH   "Path to vendor/vst3sdk checkout")
set(VXN_CLAP_WRAPPER_DIR "" CACHE PATH   "Path to vendor/clap-wrapper checkout")
set(VXN_OUTPUT_DIR "${CMAKE_BINARY_DIR}/out" CACHE PATH "Directory the finished VXN1.vst3 is copied to")

foreach(_req VXN_CLAP_STATIC VXN_CLAP_SDK_DIR VXN_VST3_SDK_DIR VXN_CLAP_WRAPPER_DIR)
  if("${${_req}}" STREQUAL "")
    message(FATAL_ERROR "vxn1-wrapper: ${_req} must be set (see CMakeLists.txt header).")
  endif()
endforeach()

# ---------------------------------------------------------------------------
# clap-wrapper configuration: fully offline, bundled, VST3 only.
# ---------------------------------------------------------------------------
# SDK roots point at our vendored submodules (ticket 0009) rather than the
# wrapper's own dependency resolution, keeping the SDK pins independent of the
# wrapper's release cadence. DOWNLOAD_DEPENDENCIES + FETCHCONTENT_FULLY_DISCONNECTED
# guarantee no network access once the submodules are checked out.
set(CLAP_SDK_ROOT "${VXN_CLAP_SDK_DIR}" CACHE STRING "" FORCE)
set(VST3_SDK_ROOT "${VXN_VST3_SDK_DIR}" CACHE STRING "" FORCE)
set(CLAP_WRAPPER_DOWNLOAD_DEPENDENCIES FALSE CACHE BOOL "" FORCE)
set(FETCHCONTENT_FULLY_DISCONNECTED    ON    CACHE BOOL "" FORCE)
set(CLAP_WRAPPER_BUILD_AUV2            FALSE CACHE BOOL "" FORCE)
set(CLAP_WRAPPER_BUILD_STANDALONE      FALSE CACHE BOOL "" FORCE)
set(CLAP_WRAPPER_BUILD_TESTS           FALSE CACHE BOOL "" FORCE)
set(CLAP_WRAPPER_OUTPUT_NAME "VXN1" CACHE STRING "" FORCE)

# Brings in the wrapper's helper functions and SDK targets (`clap`, later
# `base-sdk-vst3`). Not top-level, so the wrapper does not build its own
# default module — we assemble ours below.
add_subdirectory("${VXN_CLAP_WRAPPER_DIR}" clap-wrapper-build EXCLUDE_FROM_ALL)

# ---------------------------------------------------------------------------
# The VST3 module: clap-wrapper VST3 glue + our static CLAP.
# ---------------------------------------------------------------------------
# target_add_vst3_wrapper adds the wrapper's export-entry source and links the
# per-target VST3 wrapper library; we add no entry source of our own. The
# wrapper locates the CLAP entry at runtime by looking up an exported
# `clap_entry` symbol in its own binary (dlsym / CFBundleGetDataPointerForName /
# GetProcAddress). clack already emits `#[no_mangle] pub static clap_entry`; the
# force_load below pulls it (and the rest of the engine) out of the archive —
# nothing references it at link time, so without whole-archive it would be
# stripped and the wrapper would find an empty factory.
add_library(vxn1_vst3 MODULE)

target_add_vst3_wrapper(
  TARGET vxn1_vst3
  OUTPUT_NAME "VXN1"
  BUNDLE_IDENTIFIER "labs.vulpus.vxn1.vst3"
  BUNDLE_VERSION "1.0"
)

foreach(_archive ${VXN_CLAP_STATIC})
  if(APPLE)
    target_link_options(vxn1_vst3 PRIVATE "-Wl,-force_load,${_archive}")
  elseif(WIN32)
    # MSVC: the archive basename (no path) is the library identifier.
    target_link_options(vxn1_vst3 PRIVATE "/WHOLEARCHIVE:${_archive}")
    # rustc bundles a short-import stub for each native dependency (ntdll,
    # etc.) as a separate archive member, and repeats them across crates.
    # /WHOLEARCHIVE force-includes every copy, so the duplicated import
    # descriptors (__IMPORT_DESCRIPTOR_ntdll, …) collide (LNK2005/LNK1169).
    # /FORCE:MULTIPLE keeps the first definition of each and links; the
    # duplicates are byte-identical import thunks, so this is safe here.
    target_link_options(vxn1_vst3 PRIVATE "/FORCE:MULTIPLE")
  else() # Linux / other ELF
    target_link_options(vxn1_vst3 PRIVATE
      "-Wl,--whole-archive" "${_archive}" "-Wl,--no-whole-archive")
  endif()
endforeach()

# A Rust cdylib records its framework/dylib dependencies; a staticlib does not,
# so the engine's transitive native deps (the wry webview faceplate, objc,
# CoreFoundation, …) must be named explicitly here. Kept in sync with
# `otool -L libvxn_clap.dylib` on the working CLAP build.
if(APPLE)
  target_link_libraries(vxn1_vst3 PRIVATE
    "-framework AppKit"
    "-framework WebKit"
    "-framework QuartzCore"
    "-framework CoreGraphics"
    "-framework Foundation"
    "-framework CoreFoundation"
    "-liconv")
elseif(WIN32)
  # wry's webview (window subclassing, PROPVARIANT/VARIANT marshalling, memory
  # streams) and windows_core's error/variant helpers pull symbols from libs a
  # staticlib doesn't record: comctl32 (SetWindowSubclass/DefSubclassProc),
  # shlwapi (SHCreateMemStream), propsys (PropVariantTo*/VariantTo*), and
  # runtimeobject (RoOriginateErrorW). Kept in sync with the working CLAP link.
  target_link_libraries(vxn1_vst3 PRIVATE
    ws2_32 userenv ntdll bcrypt
    user32 gdi32 shell32 ole32 oleaut32 advapi32 version
    comctl32 shlwapi propsys runtimeobject)
elseif(UNIX)
  target_link_libraries(vxn1_vst3 PRIVATE dl pthread m)
endif()

# ---------------------------------------------------------------------------
# Stage the finished bundle into VXN_OUTPUT_DIR.
# ---------------------------------------------------------------------------
# On macOS the artifact is a MODULE bundle directory; elsewhere the wrapper
# writes a VXN1.vst3 file (Windows single-file) or folder (Windows-folder /
# Linux) into the build tree.
add_custom_command(TARGET vxn1_vst3 POST_BUILD
  COMMAND ${CMAKE_COMMAND} -E make_directory "${VXN_OUTPUT_DIR}")

if(APPLE)
  add_custom_command(TARGET vxn1_vst3 POST_BUILD
    COMMAND ${CMAKE_COMMAND} -E rm -rf "${VXN_OUTPUT_DIR}/VXN1.vst3"
    COMMAND ${CMAKE_COMMAND} -E copy_directory
            "$<TARGET_BUNDLE_DIR:vxn1_vst3>" "${VXN_OUTPUT_DIR}/VXN1.vst3"
    COMMENT "vxn1-wrapper: staging VXN1.vst3 → ${VXN_OUTPUT_DIR}")
else()
  add_custom_command(TARGET vxn1_vst3 POST_BUILD
    COMMAND ${CMAKE_COMMAND} -E rm -rf "${VXN_OUTPUT_DIR}/VXN1.vst3"
    COMMAND ${CMAKE_COMMAND} -E copy_directory_if_different
            "$<TARGET_FILE_DIR:vxn1_vst3>/../.." "${VXN_OUTPUT_DIR}/VXN1.vst3"
    COMMENT "vxn1-wrapper: staging VXN1.vst3 → ${VXN_OUTPUT_DIR}")
endif()
