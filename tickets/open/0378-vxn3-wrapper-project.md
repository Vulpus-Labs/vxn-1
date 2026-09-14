---
id: "0378"
product: vxn-3
title: "vxn-3 gains a wrapper project — VST3 and AU in one stroke"
priority: medium
created: 2026-09-10
epic: E051
depends: ["0377"]
---

## Summary

vxn-3 is CLAP-only: no `wrapper/` directory, `vst3: None` in its xtask, and
`crate-type = ["cdylib", "rlib"]` with no `staticlib`
([vxn3-clap/Cargo.toml:13](../../vxn-3/crates/vxn3-clap/Cargo.toml#L13)). Both
wrapper formats need the same three things, so it gets VST3 and AU together.
Twelfth ticket of [E051](../../epics/open/E051-audio-unit-distribution.md).

## Acceptance criteria

- [ ] `staticlib` added to `vxn3-clap`'s crate-type; `libvxn3_clap.a` builds on
      macOS and Windows.
- [ ] `vxn-3/wrapper/CMakeLists.txt` calls
      [0377](0377-shared-wrapper-cmake.md)'s `vxn_add_wrapper_targets` with
      `FORMATS vst3 auv2` and subtype `Vxn3`.
- [ ] `Product::vst3` and `Product::au` populated in vxn-3's xtask;
      `--format clap,vst3,au --universal` produces all three, signed.
- [ ] Force-load assertion (`labs.vulpus.vxn3` in the binary) passes for both
      wrapped formats; `auval -v aumu Vxn3 Vlps` passes.
- [ ] `VXN3.vst3` builds on Windows and loads.
- [ ] Faceplate live in both, which is [0370](0370-vxn3-tick-source.md)'s
      fallback doing its job under AU — verify the lane playhead animates, not
      just that the window opens.

## Notes

vxn-3 has the largest editor surface of the range, so it is the best test of the
`request_callback` tick rate under AU: a ~100 Hz idle loop should be
indistinguishable from the 60 Hz host timer, and if it is not, that is worth
knowing before [0379](0379-vxn4-wrapper-project.md).

The native-dependency list in the shared module was tuned against vxn-2's wry
build; vxn-3 uses the same webview stack, so it should need nothing new — if it
does, that is a signal the list belongs somewhere more derived than hand-kept.
