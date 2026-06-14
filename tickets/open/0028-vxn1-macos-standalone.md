---
id: "0028"
product: vxn-2
title: "vxn-1 macOS standalone (.app) + xtask standalone"
priority: medium
created: 2026-06-13
epic: E014
depends: ["0027"]
---

## Summary

Second ticket of [E014](../../epics/open/E014-standalone-builds.md).
Produce `vxn1-standalone.app` on macOS via clap-wrapper, hosting the
built `VXN1.clap`. This is the proving ground — all paths
(wry-on-main-thread, embedded `set_parent`, note-ports) are already
exercised on macOS under DAWs.

vxn-1's `staticlib` crate-type is added by E010 0008 — reuse it here
rather than duplicating.

## Design

- Wire the 0027 scaffold to vxn-1: feed `vxn-clap`'s static archive to
  `target_add_standalone_wrapper` (bundled mode), set bundle id / app
  name / window size.
- Add an `xtask standalone` subcommand to vxn-1's xtask that:
  1. builds the `vxn-clap` staticlib slice(s),
  2. invokes CMake to build + assemble the `.app` (CLAP linked in).
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
- The CLAP is statically linked into the `.app` — no separate `.clap`
  file dependency.
- No change to vxn-1 DSP, params, editor, or the CLAP behaviour (only
  `vxn-clap`'s crate-type gains `staticlib`, via E010 0008).
