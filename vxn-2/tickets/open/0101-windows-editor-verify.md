---
id: "0101"
title: "Windows editor / window-handling verification"
priority: high
created: 2026-06-13
epic: E009
depends: ["0100"]
---

## Summary

Fourth ticket of [E009](../../epics/open/E009-windows-parity.md). Load
the CI-built `VXN2.clap` in a Windows CLAP host and confirm the WebView2
editor actually mounts and works. The window-handling code exists but
has never executed on Windows — loading without errors is not the same
as the editor opening.

## Design

- Load `VXN2.clap` in a Windows CLAP host (e.g. Bitwig, Reaper with
  CLAP, or the `clap-host` reference host).
- Verify, in order:
  1. Plugin instantiates and audio renders (notes sound).
  2. The editor **opens and renders** the faceplate — guard against the
     "Windows no-UI" bug class documented in vxn-1's `gui.rs` (a missing
     per-OS parent-handle branch makes the accessor return `None` and
     the editor silently never opens). vxn-2's `set_parent` has the
     `as_win32_hwnd` branch; confirm it executes.
  3. Param IPC round-trips (move a knob → engine responds → automation
     echo updates the UI).
  4. The native text-input popup (`WS_POPUP`, owner-anchored to the host
     HWND) opens and accepts Enter/Esc — it exists to bypass the host's
     transport-key swallow.
- Confirm `ensure_webview2_data_dir` (vendor "Vulpus" / product "VXN2")
  behaves, and document the **WebView2 runtime prerequisite** (ships by
  default on current Win10/11).

## Acceptance

- Documented pass (screenshots / notes) for all four checks above in a
  Windows CLAP host.
- WebView2 runtime prereq recorded (where it comes from, behaviour on a
  clean machine).
- Any defect found is filed as a follow-up ticket or fixed here if
  small.
