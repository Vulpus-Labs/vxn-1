---
id: "0105"
title: "vxn-2 macOS standalone (.app)"
priority: medium
created: 2026-06-13
epic: E010
depends: ["0104"]
---

## Summary

Third ticket of [E010](../../epics/open/E010-standalone-builds.md).
Produce `vxn2-standalone.app`, reusing the pattern proven in 0104.
Mostly configuration — different `.clap`, bundle id, window size.

Depends on `vxn2-clap` having the `staticlib` crate-type (added in
0103).

## Design

- Point the standalone scaffold at `vxn2-clap`'s static archive
  (bundled mode).
- Add the `standalone` subcommand to vxn-2's xtask (or share a common
  helper with vxn-1 if the CMake driver generalises cleanly).
- Use vxn-2's editor window dimensions (`vxn2-ui-web` `EDITOR_WIDTH` /
  `EDITOR_HEIGHT`).

## Acceptance

- `vxn2-standalone.app` launches, opens the editor, makes sound from a
  MIDI keyboard, and exposes device selection.
- The CLAP is statically linked into the `.app` — no separate `.clap`
  file dependency.
- No change to vxn-2 DSP, params, or editor (only `vxn2-clap`'s
  crate-type gains `staticlib`).
