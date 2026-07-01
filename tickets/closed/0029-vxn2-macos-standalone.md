---
id: "0029"
product: vxn-2
title: "vxn-2 macOS standalone (.app)"
priority: medium
created: 2026-06-13
epic: E014
depends: ["0028"]
---

## Summary

Third ticket of [E014](../../epics/open/E014-standalone-builds.md).
Produce `vxn2-standalone.app`, reusing the pattern proven in 0028.
Mostly configuration — different `.clap`, bundle id, window size.

Depends on `vxn2-clap` having the `staticlib` crate-type (added in
0027).

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

## Close-out (2026-07-01)

- [vxn-2/xtask/src/main.rs](../../vxn-2/xtask/src/main.rs): `"standalone"` arm added (line 37); `standalone(release)` function (line 229) builds `vxn2-clap` staticlib, configures `standalone/CMakeLists.txt` with `VXN_PLUGIN_NAME=VXN2` / `VXN_BUNDLE_ID=labs.vulpus.vxn2.standalone` into `target/standalone2-{profile}`.
- On macOS: copies `VXN2.app` to `target/bundled/VXN2.app`. On Windows: copies `VXN2.exe` to `target/bundled/VXN2.exe`.
- `vxn2-clap` `staticlib` crate-type added ([vxn-2/crates/vxn2-clap/Cargo.toml](../../vxn-2/crates/vxn2-clap/Cargo.toml) line 14) as part of 0027.
- Reuses shared `standalone/CMakeLists.txt` (separate build dir `standalone2-{profile}` avoids clobbering vxn-1). Runtime launch pending hardware test.
