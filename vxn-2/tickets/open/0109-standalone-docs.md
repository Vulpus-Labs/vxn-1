---
id: "0109"
title: "Docs: standalone usage + device selection + WebView2 prereq"
priority: low
created: 2026-06-13
epic: E010
depends: ["0108"]
---

## Summary

Final ticket of [E010](../../epics/open/E010-standalone-builds.md).
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
