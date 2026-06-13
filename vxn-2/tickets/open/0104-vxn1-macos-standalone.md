---
id: "0104"
title: "vxn-1 macOS standalone (.app) + xtask standalone"
priority: medium
created: 2026-06-13
epic: E010
depends: ["0103"]
---

## Summary

Second ticket of [E010](../../epics/open/E010-standalone-builds.md).
Produce `vxn1-standalone.app` on macOS via clap-wrapper, hosting the
built `VXN1.clap`. This is the proving ground — all paths
(wry-on-main-thread, embedded `set_parent`, note-ports) are already
exercised on macOS under DAWs.

## Design

- Wire the 0103 scaffold to vxn-1: point the embedded-clap location at
  the built `VXN1.clap`, set bundle id / app name / window size.
- Add an `xtask standalone` subcommand to vxn-1's xtask that:
  1. runs `bundle` to produce the `.clap`,
  2. invokes CMake to build + assemble the `.app`,
  3. embeds the `.clap` inside the `.app` (not a dev install path).
- clap-wrapper provides: RtAudio output, RtMidi input → the plugin's
  note-ports, a top-level window + menu, and the `gui` set_parent call
  the wry editor already handles.
- Confirm the `lipo` universal slices survive the standalone packaging
  (or build per-arch + `lipo` after).

## Acceptance

- `cargo xtask standalone` (or documented CMake invocation) produces
  `vxn1-standalone.app`.
- Double-click launches: editor opens, a connected MIDI keyboard makes
  sound, audio + MIDI devices are selectable.
- The hosted `VXN1.clap` is embedded in the `.app`.
- No change to vxn-1 DSP, params, editor, or the `.clap` itself.
