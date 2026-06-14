---
id: "0031"
product: vxn-2
title: "vxn-2 Windows standalone (.exe)"
priority: medium
created: 2026-06-13
epic: E014
depends: ["0029", "0030"]
---

## Summary

Fifth ticket of [E014](../../epics/open/E014-standalone-builds.md).
Produce `VXN2.exe` on Windows. **Gated on E013** — needs a verified
Windows `VXN2.clap` (E013 0025) and the Windows standalone pattern from
0030.

## Design

- Requires [E013](../../epics/open/E013-windows-parity.md) closed: a
  green Windows `VXN2.clap` whose editor is verified to open.
- Reuse the 0030 Windows standalone CMake target, pointed at
  `vxn2-clap`'s Windows static archive with vxn-2's bundle id / window
  size (bundled mode).
- Re-verify editor mount + text-input popup under clap-wrapper's HWND
  (the standalone window differs from the DAW window 0025 tested).

## Acceptance

- `VXN2.exe` launches, opens the editor, makes sound from a MIDI
  keyboard, and exposes device selection.
- Text-input popup accepts Enter/Esc.
- The CLAP is statically linked into the `.exe` — self-contained.
