---
id: "0034"
product: vxn-2
title: "vxn-1 WASM/browser feasibility spike — Synth::process in an AudioWorklet"
priority: low
created: 2026-06-14
epic: null
depends: []
---

## Summary

Prove (or kill) a browser/WASM port of vxn-1. The synth core is already
WASM-friendly — [`Synth::process(out_l, out_r)`](../../vxn-1/crates/vxn-engine/src/lib.rs#L441)
is allocation-free, fixed-size buffers, pure scalar `f32` (no NEON/x86
intrinsics in the DSP kernels — vectorisation is compiler auto-vec only).
The friction is entirely in the plugin shell (clack/CLAP, wry, baseview,
raw-window-handle) which a web build replaces wholesale with Web Audio.

This is a throwaway spike, not a product port. Goal: de-risk the two
things that actually bite — the AudioWorklet+WASM render path and
denormals — by playing **one note** in a browser tab. Outcome is a
go/no-go writeup, not shippable code.

## Design

- New throwaway crate (or `cargo` target) compiling **vxn-engine only**
  to `wasm32-unknown-unknown` via `wasm-bindgen`. Fork at the `Synth`
  boundary; do NOT pull in vxn-clap / vxn-ui-web / wry.
- Export `Synth::process` to JS: a worklet-side WASM module that fills an
  interleaved/planar block per `process()` callback.
- Host it in an `AudioWorkletProcessor`. Note: worklet runs in a separate
  realtime thread — WASM module must be instantiated inside the worklet,
  and main-thread→worklet param/note passing goes over `port.postMessage`
  (or `SharedArrayBuffer`, which needs COOP/COEP cross-origin-isolation
  headers — flag this if SAB is used).
- Trigger one note-on from a button (hardcoded note, no Web MIDI yet).
- **Denormals:** `ScopedFlushToZero` is a no-op on wasm (FTZ is
  arch-gated in [vxn-core-utils/src/ftz.rs](../../crates/vxn-core-utils/src/ftz.rs)).
  Filter/reverb feedback tails will hit denormal CPU cliffs. Measure it;
  if it bites, prototype a manual flush (DC offset / hard-flush in
  feedback paths) and note the scope of DSP change required.

Out of scope: UI faceplate port, Web MIDI, preset I/O (IndexedDB),
mod/automation, multi-voice perf tuning, build pipeline. Those only
matter if this spike says go.

## Acceptance criteria

- [ ] `vxn-engine` compiles to `wasm32-unknown-unknown` (document any
      source changes needed — std/fs gating, etc.).
- [ ] A browser tab plays one sustained note rendered by `Synth::process`
      running inside an `AudioWorkletProcessor`. Audible, no glitches at
      idle.
- [ ] Denormal behaviour characterised: measure worklet CPU on a decaying
      filter/reverb tail with FTZ absent; record whether a manual flush is
      required and roughly how invasive it'd be.
- [ ] Short writeup: go/no-go, the AudioWorklet+WASM gotchas hit, denormal
      verdict, and a rough effort estimate for the full port (engine →
      worklet glue → UI rewire → Web MIDI → IndexedDB presets).

## Notes

- Effort: ~days for the spike itself; full port estimated ~5–6 weeks
  (engine/DSP + denormal fix ~1–2wk, AudioWorklet+wasm-bindgen glue ~2wk
  — the hard part, UI/MIDI/preset rewire ~1–2wk).
- The UI is already message-based JSON opcodes
  ([vxn-core-app/src/events.rs](../../crates/vxn-core-app/src/events.rs)),
  so a future UI port reuses the opcode protocol over `postMessage`
  instead of wry `evaluate_script` — but that's a later ticket.
- `priority: low` — exploratory, no downstream dependents.
