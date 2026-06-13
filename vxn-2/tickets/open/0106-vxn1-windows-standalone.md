---
id: "0106"
title: "vxn-1 Windows standalone (.exe)"
priority: medium
created: 2026-06-13
epic: E010
depends: ["0104"]
---

## Summary

Fourth ticket of [E010](../../epics/open/E010-standalone-builds.md).
Produce `VXN1.exe` on Windows via clap-wrapper, hosting `VXN1.clap`.
vxn-1 is already Windows-capable as a plugin, so no E009 dependency for
this synth.

## Design

- Build `VXN1.clap` on Windows (existing vxn-1 pipeline), then run the
  clap-wrapper standalone CMake target for Windows (RtAudio / RtMidi /
  WebView2; `windows_standalone.cpp` provides the window + menu).
- The standalone's window is clap-wrapper's HWND, not a DAW's — confirm
  the wry editor and the `WS_POPUP` text-input popup anchor to it
  correctly.
- Embed `VXN1.clap` next to / inside the `.exe` distribution; point the
  hosted-clap location at the bundled copy.

## Acceptance

- `VXN1.exe` launches, opens the WebView2 editor, makes sound from a
  MIDI keyboard, and exposes audio/MIDI device selection.
- Text-input popup accepts Enter/Esc.
- WebView2 runtime prereq documented (carried from E009 0101).
