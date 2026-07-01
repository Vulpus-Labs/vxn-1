---
id: "0033"
product: vxn-2
title: "Docs: standalone usage + device selection + WebView2 prereq"
priority: low
created: 2026-06-13
epic: E014
depends: ["0032"]
---

## Summary

Final ticket of [E014](../../epics/open/E014-standalone-builds.md).
Document the standalone apps for end users.

## Design

- Cover, per OS:
  - launching the standalone (`.app` on macOS, `.exe` on Windows),
  - selecting audio output + MIDI input devices (clap-wrapper's
    standard standalone menu),
  - the **WebView2 runtime** prerequisite on Windows (ships by default
    on current Win10/11; where to get it otherwise),
  - any first-launch OS prompts (macOS Gatekeeper, mic/audio
    permissions if applicable).
- Slot into the existing docs layout (vxn-1 has a `docs/` mdbook + PDF
  manual; mirror for vxn-2 or add a shared standalone section).

## Acceptance

- User-facing docs describe launch, device selection, and the WebView2
  prereq for both synths on both OSes.

## Close-out (2026-07-01)

- [vxn-1/docs/src/standalone.md](../../vxn-1/docs/src/standalone.md): new page covering build commands (VXN1 `--universal` + VXN2), output table (macOS `.app` / Windows `.exe`), launch instructions per OS, device selection (Audio/MIDI menus from clap-wrapper), WebView2 prereq (ships Win10 2004+/Win11; download link for older; standalone still launches without editor on missing runtime), macOS Gatekeeper (`xattr -d/rd com.apple.quarantine`), and audio permission on first launch.
- [vxn-1/docs/src/SUMMARY.md](../../vxn-1/docs/src/SUMMARY.md) line 8: `- [Standalone apps](standalone.md)` added under "Getting started" after "Installing VXN1". Covers both synths (VXN1 + VXN2) in all examples and the output table.
