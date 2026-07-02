---
id: "0031"
product: vxn-2
title: "vxn-2 Windows standalone (.exe)"
priority: medium
status: shelved
created: 2026-06-13
shelved: 2026-07-02
epic: E014
depends: ["0029", "0030"]
---

> **SHELVED 2026-07-02** with the rest of [E014](../../epics/closed/E014-standalone-builds.md).
> Standalone builds dropped for now — see the epic for the clap-wrapper
> macOS blockers (dead faceplate: no `timer-support`; TCC mic crash) and
> what was removed. Revive with the epic.


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
