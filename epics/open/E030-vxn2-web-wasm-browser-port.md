---
id: E030
product: vxn-2
title: "vxn-2 web/wasm browser port"
status: open
created: 2026-06-30
---

> The web analogue of vxn-2's CLAP build, following the proven vxn-1
> blueprint (epics E015–E020, ADR 0009). Unlike vxn-1 this epic skips the
> spike-scaffold rhythm: the 2026-06-30 compile spike confirmed
> `vxn2-dsp` + `vxn2-engine` + `vxn2-app` build for
> `wasm32-unknown-unknown` (release + SIMD128) with **zero source
> changes**, so the architecture is known-good before scoping. The work
> is glue, build pipeline, and UI rewire — not core changes.

## Goal

Ship vxn-2 as a browser instrument reachable from a static URL, feature-
matched to the native CLAP build: faceplate UI, the full param/op/matrix
surface, factory bank, user presets, and MIDI/keyboard input — all driven
by the same `vxn2-engine` / `vxn-app` controller that drives the plugin.

When this epic closes:

- `cargo xtask web` (vxn-2) produces a self-contained `dist/` (two wasm
  modules, JS glue, worklet, faceplate assets, baked factory bank) in one
  command.
- The served page boots an AudioContext, runs the engine in an
  AudioWorklet, and plays notes from MIDI / computer keyboard.
- The faceplate (existing `vxn2-ui-web` assets) hydrates and round-trips
  every param/op/matrix gesture over `postMessage` + `SharedArrayBuffer`.
- User presets persist in IndexedDB; state autosaves; patches export /
  import / share via URL.
- A documented COOP/COEP hosting recipe is validated on one static host.

## Architecture (mirrors vxn-1 ADR 0009)

Two-module wasm design:

1. **`vxn2-wasm`** (new cdylib) — engine in the AudioWorklet. Per-quantum
   render loop + binary event codec. Ports `vxn-wasm/src/host.rs` and
   `codec.rs`, swapping `vxn-engine`→`vxn2-engine`.
2. **`vxn2-web-controller`** (new cdylib) — `vxn-app` controller on the
   main thread, reused verbatim (same arbiter as native). Ports
   `vxn-web-controller`.

They communicate over `SharedArrayBuffer`: an SPSC event ring (note +
param events, 16-byte slots) and a lock-free param store (block-start
folding). JS glue (~13 `.mjs`) ports from `vxn-wasm/web/`.

## Scope

**In:** the two wasm crates; SAB transport JS; AudioWorklet + coordinator
bootstrap; rewire of `vxn2-ui-web` assets from wry `evaluate_script` to
`postMessage`; `xtask web` build pipeline (gen-web-page bin, bake-factory,
dist assembly, `_headers`); IndexedDB preset persistence + autosave +
patch-io; MIDI + keyboard input; perf/cross-browser hardening + hosting
doc.

**Out:** changes to `vxn2-dsp`/`vxn2-engine`/`vxn2-app` core (spike proved
none needed); the native CLAP build (`vxn2-clap` stays as-is, excluded
from wasm); new DSP features.

## vxn-2 deltas vs the vxn-1 blueprint

- **More params** (~250+, ops×per-op vs 165) → larger param-store SAB,
  same codec. Watch the 16-bit param-index field still fits.
- **Preset format** is TOML+serde+`include_dir` (vxn-1 was a binary blob)
  → IndexedDB adapter wraps the existing `vxn2-engine` codec; `std::fs`
  paths are already target-gated no-ops on wasm.
- **Extra surface**: phaser FX + `ks-graph`/`eg-graph` panels — already in
  `vxn2-dsp` and the assets, just carried along.
- **Mod-matrix coherence** validation already exported as JSON
  server-side (`matrix_lists_json`).

## Planned tickets

- [ ] `vxn2-wasm` engine cdylib: C-ABI host render loop + event codec
      (port `vxn-wasm` host.rs/codec.rs).
- [ ] `vxn2-web-controller` cdylib: main-thread controller wasm (port
      `vxn-web-controller`).
- [ ] SAB transport JS: event-ring + param-store + event-codec
      (JS side of the codec, kept in sync with the Rust def).
- [ ] AudioWorklet + coordinator bootstrap: processor, host-runner,
      audio-host, AudioContext lifecycle (autoplay unlock, suspend/resume).
- [ ] Faceplate rewire: `vxn2-ui-web` assets from wry `evaluate_script`
      to `postMessage` + faceplate-bridge; gen-web-page bin.
- [ ] `cargo xtask web` build pipeline: build both wasms (release,
      SIMD128), bake factory bank, assemble `dist/`, emit COOP/COEP
      `_headers`; COOP/COEP dev server.
- [ ] Browser persistence: IndexedDB user presets + state autosave +
      patch export/import/URL-share.
- [ ] Input adapters: Web MIDI + computer-keyboard → ring producers.

## Risks

- **Param-store width.** ~250+ params vs vxn-1's 165 — verify codec field
  widths and SAB sizing before building transport.
- **Autoplay / isolation.** Same as vxn-1: AudioContext gated on user
  gesture; COOP/COEP breaks iframing — document it.
- **Toolchain.** wasm32 build needs rustup, not this box's Homebrew rust;
  `xtask web` must force the right `RUSTC`/PATH or pin a
  `rust-toolchain.toml` (memory `wasm-build-toolchain`).
- **Codec drift.** The 16-byte event format has one Rust def + one JS def;
  they must stay byte-identical (vxn-1 hit this — keep a shared spec).

## Acceptance

- `cargo xtask web` yields a servable `dist/` in one command.
- Served page reaches "audio live" after a user gesture and plays from
  MIDI/keyboard.
- Every faceplate param/op/matrix gesture round-trips; factory bank loads;
  user presets persist across reloads.
- `SharedArrayBuffer` available (isolation headers verified); hosting
  recipe validated on one static host.
