---
id: "0025"
product: vxn-2
title: "Windows editor / window-handling verification"
priority: high
created: 2026-06-13
epic: E013
depends: ["0024"]
---

## Summary

Fourth ticket of [E013](../../epics/open/E013-windows-parity.md). Load
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

## Close-out (2026-08-11)

**Verified on Windows by the maintainer; all four checks passed.** This ticket
is a manual-verification deliverable — no code change was needed, and none was
made.

- **Instantiate + audio.** `VXN2.clap` loads in a Windows CLAP host and renders;
  notes sound.
- **Editor opens and renders.** The faceplate mounts — i.e. `set_parent`'s
  `as_win32_hwnd` branch executes, and the "Windows no-UI" bug class documented
  in vxn-1's `gui.rs` (missing per-OS parent-handle branch → accessor returns
  `None` → editor silently never opens) does **not** reproduce here.
- **Param IPC round-trips.** Moving a control reaches the engine and the
  automation echo updates the UI.
- **Native text-input popup.** The owner-anchored `WS_POPUP` opens and accepts
  Enter/Esc, so the host's transport-key swallow is bypassed as designed.

`ensure_webview2_data_dir` (vendor "Vulpus" / product "VXN2") behaved. The
WebView2 runtime is a prerequisite; it ships by default on current Win10/11, so
no bundled installer is needed for those targets.

No defects found, so no follow-up ticket filed. The verification was performed
interactively against a live Windows host — the per-check screenshots/notes the
Acceptance section asks for live with the maintainer rather than in the repo,
which is the one respect in which this close-out is thinner than the ticket
specified.

