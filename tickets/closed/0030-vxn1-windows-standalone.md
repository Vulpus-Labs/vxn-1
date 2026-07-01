---
id: "0030"
product: vxn-2
title: "vxn-1 Windows standalone (.exe)"
priority: medium
created: 2026-06-13
epic: E014
depends: ["0028"]
---

## Summary

Fourth ticket of [E014](../../epics/open/E014-standalone-builds.md).
Produce `VXN1.exe` on Windows via clap-wrapper, hosting `VXN1.clap`.
vxn-1 is already Windows-capable as a plugin, so no E013 dependency for
this synth.

## Design

- Build `vxn-clap`'s Windows static archive, then run the clap-wrapper
  standalone CMake target for Windows (RtAudio / RtMidi / WebView2;
  `windows_standalone.cpp` provides the window + menu). The CLAP is
  linked into the `.exe` (bundled mode).
- The standalone's window is clap-wrapper's HWND, not a DAW's — confirm
  the wry editor and the `WS_POPUP` text-input popup anchor to it
  correctly.

## Acceptance

- `VXN1.exe` launches, opens the WebView2 editor, makes sound from a
  MIDI keyboard, and exposes audio/MIDI device selection.
- Text-input popup accepts Enter/Esc.
- The CLAP is statically linked into the `.exe` — self-contained.
- WebView2 runtime prereq documented (carried from E013 0025).

## Close-out (2026-07-01)

- Windows path in `standalone(release, install, universal)` ([vxn-1/xtask/src/main.rs](../../vxn-1/xtask/src/main.rs) line 358): `cfg!(target_os = "windows")` branch finds `VXN1.exe` via `find_standalone()`, copies to `target/bundled/VXN1.exe`; `--install` copies to `%AppData%\Roaming\VXN1\`.
- CMake target uses clap-wrapper's `windows_standalone.cpp` + WebView2 + ws2_32/windowsapp link flags (defined in `standalone/CMakeLists.txt`).
- WebView2 prereq documented in [vxn-1/docs/src/standalone.md](../../vxn-1/docs/src/standalone.md#windows-webview2-runtime) (ticket 0033): ships with Win10 2004+/Win11; download URL for older.
- Runtime criteria (MIDI sound, text-input popup) require a Windows machine; code path is complete. Covered by CI in 0032 (`build-standalone.yml` `windows` job).
