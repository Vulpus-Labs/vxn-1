---
id: "0110"
title: Wrapper CMakeLists (bundled mode, mac + win)
priority: high
created: 2026-06-08
epic: E020
---

## Summary

Author `vxn-1/wrapper/CMakeLists.txt` that drives
clap-wrapper in bundled single-binary mode, statically linking
the `vxn-clap` archive produced by ticket 0108 and producing
`VXN1.vst3` for the host platform. Cross-platform (mac +
win); the Windows-specific xtask wiring lives in ticket 0112,
but the CMake itself must already work on both.

Per ADR 0008 §1, §2.

## Acceptance criteria

- [ ] New file `vxn-1/wrapper/CMakeLists.txt`. Minimum CMake
      3.21. Project name `vxn1-wrapper`. Languages C, CXX.
- [ ] Consumes the following cache variables (passed by
      xtask in tickets 0111 / 0112):
      - `VXN_CLAP_STATIC` — path to `libvxn_clap.a` /
        `vxn_clap.lib` (or `;`-separated list for universal
        mac).
      - `VXN_VST3_SDK_DIR` — path to `vendor/vst3sdk`
        checkout.
      - `VXN_CLAP_WRAPPER_DIR` — path to
        `vendor/clap-wrapper` checkout.
      - `VXN_OUTPUT_DIR` — where the `.vst3` should land.
      Optional:
      - `CMAKE_OSX_ARCHITECTURES` — `"arm64;x86_64"` for
        universal macOS builds.
- [ ] Sets `CLAP_WRAPPER_OUTPUT_NAME=VXN1`,
      `CLAP_WRAPPER_BUILD_AUV2=FALSE`,
      `CLAP_WRAPPER_DOWNLOAD_DEPENDENCIES=FALSE`. Points
      clap-wrapper's `VST3_SDK_ROOT` (or equivalent) at
      `VXN_VST3_SDK_DIR` so the wrapper's bundled SDK is
      bypassed.
- [ ] `add_subdirectory(${VXN_CLAP_WRAPPER_DIR})` brings in
      the wrapper's targets. The local `vxn1-wrapper` target
      links the static archive(s) from `VXN_CLAP_STATIC`,
      including platform whole-archive flags so `clap_entry`
      survives strip:
      - macOS: `-Wl,-force_load,<path>` per archive.
      - Linux: `-Wl,--whole-archive <paths> -Wl,--no-whole-
        archive`.
      - Windows: `/WHOLEARCHIVE:vxn_clap` linker flag.
- [ ] After `cmake --build` the resulting `VXN1.vst3` (mac
      bundle directory, win folder-style bundle) is copied
      to `VXN_OUTPUT_DIR` via a CMake `install()` or post-
      build step. Confirm the bundle is well-formed:
      - macOS: `Contents/Info.plist`,
        `Contents/MacOS/VXN1`, `Contents/PkgInfo`.
      - Windows: `Contents/x86_64-win/VXN1.vst3` shared lib
        inside the bundle dir.
- [ ] `cmake -S vxn-1/wrapper -B target/wrapper-test
      -DVXN_CLAP_STATIC=$(cargo build path)
      -DVXN_VST3_SDK_DIR=vendor/vst3sdk
      -DVXN_CLAP_WRAPPER_DIR=vendor/clap-wrapper
      -DVXN_OUTPUT_DIR=target/wrapper-test/out` followed by
      `cmake --build target/wrapper-test` succeeds on macOS
      and produces a loadable `VXN1.vst3`.
- [ ] Same invocation succeeds on Windows under MSVC 2022 +
      Ninja and produces a loadable bundle (validation lives
      in ticket 0113; here we only need successful build +
      well-formed bundle layout).
- [ ] No bundled dependency download — the build is fully
      offline once submodules are checked out.

## Notes

Bundled mode is upstream-supported but the variable names have
churned across clap-wrapper releases. Check the wrapper's own
`CMakeLists.txt` in the pinned tag (ticket 0109) for the
current names; this ticket's bullet list above describes the
intent, not the exact symbol literals.

If `force_load` / `whole-archive` proves unnecessary in
practice (i.e. linker keeps `clap_entry` because clap-wrapper
itself references it through its translation glue), drop the
flags — but verify the produced binary actually exposes the
CLAP entry the wrapper expects. Empty CLAP discovery from the
wrapper side is the failure mode to watch for.

macOS code signing is deliberately out of scope here; an
unsigned `.vst3` still loads in DAWs in dev. A future `xtask
sign` task will codesign for distribution.

Don't try to make this CMake build against the existing
`libvxn_clap.dylib` (i.e. external CLAP mode) — that is the
mode we rejected in ADR 0008 §2.

If clap-wrapper's pinned-tag CMake doesn't expose
`CLAP_WRAPPER_DOWNLOAD_DEPENDENCIES` cleanly, set
`FETCHCONTENT_FULLY_DISCONNECTED=ON` as a belt-and-braces
guard against silent downloads.
