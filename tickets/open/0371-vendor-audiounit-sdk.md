---
id: "0371"
product: monorepo
title: "Vendor apple/AudioUnitSDK as a submodule — the AU build must stay offline"
priority: high
created: 2026-09-10
epic: E051
depends: []
---

## Summary

clap-wrapper's AUv2 flavour needs Apple's AudioUnitSDK, which it resolves three
ways ([base_sdks.cmake:218](../../vendor/clap-wrapper/cmake/base_sdks.cmake#L218)):
an explicit `AUDIOUNIT_SDK_ROOT`, a CPM download, or a source search. Our
wrapper builds are deliberately offline —
`CLAP_WRAPPER_DOWNLOAD_DEPENDENCIES FALSE` and
`FETCHCONTENT_FULLY_DISCONNECTED ON`
([vxn-2/wrapper/CMakeLists.txt:90-91](../../vxn-2/wrapper/CMakeLists.txt#L90-L91))
— so the download path is closed and the SDK has to be vendored like the other
three. Fifth ticket of
[E051](../../epics/open/E051-audio-unit-distribution.md).

## Acceptance criteria

- [ ] `vendor/AudioUnitSDK` submodule at tag `AudioUnitSDK-1.1.0` (the version
      clap-wrapper's CPM path pins), with `.gitmodules` updated.
- [ ] The submodule preflight in
      [vxn-xtask-common/src/lib.rs:628](../../crates/vxn-xtask-common/src/lib.rs#L628)
      checks it alongside `vendor/clap`, `vendor/clap-wrapper` and
      `vendor/vst3sdk`, and fails with the same actionable message.
- [ ] `README.md` / `RELEASING.md` submodule-init instructions updated.
- [ ] CI checkout steps fetch it (`submodules: recursive` already covers this —
      confirm rather than assume).

- [ ] Windows and Linux builds are unaffected: `guarantee_auv2sdk` returns an
      INTERFACE stub off Apple
      ([base_sdks.cmake:223](../../vendor/clap-wrapper/cmake/base_sdks.cmake#L223)),
      so the preflight must not demand the SDK on those hosts.

## Notes

The SDK is only ~40 source files and is BSD-licensed; vendoring it is the same
bargain already taken for the VST3 SDK. Licence attribution goes wherever the
VST3 SDK's already lives.
