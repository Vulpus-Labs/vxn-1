---
id: "0091"
product: vxn-2
title: "Cross-browser / -device support matrix + fallbacks doc"
priority: high
created: 2026-06-22
epic: E020
depends: ["0087", "0088"]
---

## Summary

Fifth ticket of [E020](../../epics/open/E020-web-perf-crossbrowser-ship.md).
Publishes the support matrix: which browsers/devices run the port, what each
needs (AudioWorklet, SharedArrayBuffer/Atomics with COOP/COEP, Web MIDI,
storage), where each falls short, and the documented fallback. This is the
ticket that turns "it works on my Chrome" into a shippable claim with stated
limits.

## Design

- **Capabilities under test, grounded in what the port actually uses.**
  - **AudioWorklet + raw `WebAssembly.instantiate`** (no wasm-bindgen) — the
    whole render path ([vxn-processor-0038.js](../../vxn-1/crates/vxn-wasm/web/vxn-processor-0038.js)).
  - **SharedArrayBuffer + Atomics** for the 0035 event ring + 0039 param store;
    these require cross-origin isolation, which the bundle bakes via the
    `_headers` file and `serve-coep.mjs`
    ([xtask main.rs:241-246, web_dist_headers:320-325](../../vxn-1/xtask/src/main.rs#L241)).
    The 0038 harness already probes SAB/Atomics presence
    ([harness-0038.mjs:57-59](../../vxn-1/crates/vxn-wasm/harness-0038.mjs#L57)).
  - **Web MIDI** ([midi-input.mjs](../../vxn-1/crates/vxn-wasm/web/midi-input.mjs))
    — absent on Safari; the keyboard input
    ([keyboard-input.mjs](../../vxn-1/crates/vxn-wasm/web/keyboard-input.mjs)) is
    the fallback.
  - **Storage** (IndexedDB) for user presets / autosave
    ([preset-storage.mjs](../../vxn-1/crates/vxn-wasm/web/preset-storage.mjs),
    [state-autosave.mjs](../../vxn-1/crates/vxn-wasm/web/state-autosave.mjs)) —
    behaviour differs in private windows / iOS.
- **Matrix axes.** Browsers: Chrome, Firefox, Safari. Devices: desktop +
  mobile. Cells record: boots? full 16-voice glitch-free (cross-ref 0087/0088)?
  Web MIDI? storage persists? plus the chosen fallback.
- **Fallbacks doc.** A `WEB-SUPPORT.md` (sibling to the existing
  `WEB-HOSTING.md` referenced at [xtask main.rs:245](../../vxn-1/xtask/src/main.rs#L245))
  capturing the matrix + per-browser fallback (Safari: keyboard-only input,
  meter off, reduced default poly per 0088; mobile: reduced poly per 0087 mobile
  tier).
- A small headless capability-probe script can assert the *feature-detection
  logic* (e.g. "Web MIDI absent → keyboard fallback attaches"); the actual
  per-browser cells are MANUAL.

## Acceptance criteria

- [ ] (headless) A capability-probe / feature-detection test asserts the
      fallback wiring: no Web MIDI → keyboard input still attaches; no SAB →
      a clear, surfaced error (not a silent hang).
- [ ] (MANUAL) Fill every matrix cell for Chrome / Firefox / Safari × desktop /
      mobile: boots, 16-voice glitch-free (cross-ref 0087/0088), Web MIDI,
      storage persistence.
- [ ] (headless) `WEB-SUPPORT.md` is committed with the matrix + per-browser
      fallbacks and is linked from the epic/README.
- [ ] (MANUAL) Each fallback is verified on the affected browser (e.g. play via
      computer keyboard on Safari with no MIDI device).

## Notes

- Depends on 0087 (perf truth per device) and 0088 (Safari voice-count floor,
  latency behaviour) — their measurements populate the perf/glitch cells.
- Memory: `vxn1-web-safari-audioworklet` (Safari quirks concentrate here).
- Out of scope: CI/deploy (0092), PWA (0093).
