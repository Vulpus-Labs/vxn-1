---
id: "0107"
title: "vxn-2 Windows standalone (.exe)"
priority: medium
created: 2026-06-13
epic: E010
depends: ["0105", "0106"]
---

## Summary

Fifth ticket of [E010](../../epics/open/E010-standalone-builds.md).
Produce `VXN2.exe` on Windows. **Gated on E009** — needs a verified
Windows `VXN2.clap` (E009 0101) and the Windows standalone pattern from
0106.

## Design

- Requires [E009](../../epics/open/E009-windows-parity.md) closed: a
  green Windows `VXN2.clap` whose editor is verified to open.
- Reuse the 0106 Windows standalone CMake target, pointed at
  `VXN2.clap` with vxn-2's bundle id / window size.
- Re-verify editor mount + text-input popup under clap-wrapper's HWND
  (the standalone window differs from the DAW window 0101 tested).

## Acceptance

- `VXN2.exe` launches, opens the editor, makes sound from a MIDI
  keyboard, and exposes device selection.
- Text-input popup accepts Enter/Esc.
- The hosted `VXN2.clap` is bundled with the `.exe`.
