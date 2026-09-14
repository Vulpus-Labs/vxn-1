---
id: "0373"
product: vxn-2
title: "vxn-2 wrapper builds VXN2.component — AUv2 off the same static archive"
priority: high
created: 2026-09-10
epic: E051
depends: ["0371", "0372"]
---

## Summary

Add an AUv2 MODULE target to `vxn-2/wrapper` beside the existing VST3 one, fed
by the same `libvxn2_clap.a`. The wrapper already turns AUv2 off explicitly
([CMakeLists.txt:92](../../vxn-2/wrapper/CMakeLists.txt#L92)); this ticket turns
it on and assembles the second module. Seventh ticket of
[E051](../../epics/open/E051-audio-unit-distribution.md), and the one that
carries the real cost.

Driven by hand-invoked CMake for now, exactly as the wrapper header documents
for the VST3 case ([CMakeLists.txt:17-24](../../vxn-2/wrapper/CMakeLists.txt#L17-L24));
[0374](0374-xtask-format-au.md) wires it into xtask.

## Design

- `CLAP_WRAPPER_BUILD_AUV2 TRUE`, `AUDIOUNIT_SDK_ROOT` pointed at
  [0371](0371-vendor-audiounit-sdk.md)'s submodule.
- `add_library(vxn2_auv2 MODULE)` + `target_add_auv2_wrapper(...)` with the
  codes from [0372](0372-adr-au-component-identity.md), taking the **explicit
  cmake-configuration** path
  ([wrap_auv2.cmake:163](../../vendor/clap-wrapper/cmake/wrap_auv2.cmake#L163)):
  `AUV2_OUTPUT_NAME`, `AUV2_SUBTYPE_CODE`, `AUV2_MANUFACTURER_NAME`,
  `AUV2_MANUFACTURER_CODE`, `AUV2_INSTRUMENT_TYPE`. That path never loads a
  `.clap` to enumerate plugins, which is what we want for a single-binary
  static build.
- Same `-Wl,-force_load` of the fat archive and the same AppKit/WebKit/…
  framework list as the VST3 target
  ([CMakeLists.txt:122-188](../../vxn-2/wrapper/CMakeLists.txt#L122-L188)) — a
  Rust staticlib records no transitive native deps, and that list is the
  hard-won copy.
- POST_BUILD staging of `VXN2.component` into `VXN_OUTPUT_DIR`, mirroring the
  `.vst3` staging block.
- Guard the whole AU section on `APPLE`; on Windows and Linux the wrapper file
  must behave exactly as it does today.

**Universal-build hazard.** `target_add_auv2_wrapper` builds a host-side
generator executable that emits `auv2_Info.plist`, `generated_entrypoints.hxx`
and `generated_cocoaclasses.hxx`
([wrap_auv2.cmake:186](../../vendor/clap-wrapper/cmake/wrap_auv2.cmake#L186),
[build-helper.cpp:440](../../vendor/clap-wrapper/src/detail/auv2/build-helper/build-helper.cpp#L440)).
It must build **native** and run at configure/build time — if
`CMAKE_OSX_ARCHITECTURES` from our universal build leaks into it, it either
fails to run or produces nothing, and the failure surfaces as a missing
generated header rather than as anything about architecture.

## Acceptance criteria

- [ ] Manual invocation documented in the wrapper header, analogous to the VST3
      one, produces `VXN2.component` in the output dir.
- [ ] `nm`/`strings` on `VXN2.component/Contents/MacOS/VXN2` shows
      `labs.vulpus.vxn2` — the archive was force-loaded, not silently dropped.
      (Same assertion the VST3 CI leg runs; the Windows hollow-module episode
      in [0315](../closed/0315-vxn1b-war-story-sweep.md) is why "it built"
      proves nothing.)
- [ ] Builds under `--universal` (arm64 + x86_64 lipo'd fat archive) with the
      build-helper still running.
- [ ] `VXN2.vst3` still builds from the same wrapper project, unchanged.
- [ ] Windows and Linux configure and build with no AU target and no new
      requirements.

## Notes

The `.component` will not load anywhere until it is ad-hoc codesigned — that is
[0374](0374-xtask-format-au.md)'s job via the existing `codesign_bundle`, and
the failure mode is the "not a plugin" scan rejection recorded in
[[macos-plugin-codesigning]]. Expect a hand-built bundle from this ticket to be
rejected by Logic until 0374 lands; sign it manually to smoke-test.
