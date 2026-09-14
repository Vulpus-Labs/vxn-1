---
id: "0379"
product: vxn-4
title: "vxn-4 gains a wrapper project — VST3 and AU, headless for now"
priority: medium
created: 2026-09-10
epic: E051
depends: ["0377"]
---

## Summary

Same as [0378](0378-vxn3-wrapper-project.md) for vxn-4, which already has
`staticlib` in its crate-type
([vxn4-clap/Cargo.toml:15](../../vxn-4/crates/vxn4-clap/Cargo.toml#L15)) but no
wrapper project and `vst3: None`
([vxn-4/xtask/src/main.rs:45](../../vxn-4/xtask/src/main.rs#L45)). Thirteenth
ticket of [E051](../../epics/open/E051-audio-unit-distribution.md).

vxn-4 has no faceplate yet, so it ships as a playable, automatable instrument
with host-generic parameter UI in both formats. That is useful on its own and
costs nothing extra here.

## Acceptance criteria

- [ ] `vxn-4/wrapper/CMakeLists.txt` calls `vxn_add_wrapper_targets` with
      `FORMATS vst3 auv2` and subtype `Vxn4`.
- [ ] `Product::vst3` and `Product::au` populated in vxn-4's xtask;
      `--format clap,vst3,au --universal` produces all three, signed.
- [ ] Force-load assertion (`labs.vulpus.vxn4` in the binary) passes for both;
      `auval -v aumu Vxn4 Vlps` passes.
- [ ] `VXN4.vst3` builds on Windows and loads.
- [ ] Parameters appear and automate in Logic's generic view; notes sound.

## Notes

The native-dependency list is where vxn-4 genuinely differs: with no faceplate
it pulls no wry/WebKit stack, so the AppKit/WebKit/QuartzCore block the shared
module names for the others is dead weight here at best. Either the shared
function gains a `NEEDS_WEBVIEW` switch or the list stays and links harmlessly —
decide deliberately and comment the choice, because when vxn-4's editor lands
this flips back.

When that editor does land, vxn-4 also joins the
[0367](0367-tick-source-request-callback-fallback.md) `TickSource` pattern; it
is absent from that phase only because there is no tick to drive.
