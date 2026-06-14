---
id: "0038"
product: vxn-2
title: "Scaffold: worklet audio-host (port the CLAP batch loop)"
priority: high
created: 2026-06-14
epic: E015
depends: ["0037"]
---

## Summary

Make the 0035 spike permanent: a real **worklet audio-host** that owns the
engine wasm and runs the production render loop — a direct port of
`VxnAudioProcessor::process`
([vxn-clap/src/lib.rs:286-390](../../vxn-1/crates/vxn-clap/src/lib.rs#L286-L390)).
This is the web analogue of `vxn-clap`'s audio-thread half and the core
deliverable of [E015](../../epics/open/E015-web-event-driven-core.md).

## Design

Per render quantum, in the worklet:

1. Read non-automatable shared state once (key mode, split point) and
   apply: `synth.set_key_mode(...)`, `synth.set_split_point(...)`.
2. Drain the 0035 event ring, decoding records with the 0037 codec into
   batches keyed by sample offset.
3. **Slice the block at offsets** and render per slice — the CLAP loop:
   apply a batch's events, then `synth.process(&mut l[start..end],
   &mut r[start..end])`, advance, repeat; render the tail. The engine is
   unchanged.
4. Pull params from the 0039 store (lock-free read) and apply where the
   plugin's `LocalParams` would.

Package as a clean Rust audio-host wrapping `Synth` (extend or supersede
the throwaway `vxn-wasm` C-ABI shim), exporting just what the worklet
calls: instantiate, `render_quantum`, ring/store wiring. Keep the no-
wasm-bindgen, raw-`WebAssembly.instantiate` approach from 0034 (it
instantiates cleanly in the worklet scope).

## Acceptance criteria

- [ ] A worklet host renders production audio by draining the event ring
      and slicing the block at event offsets — structurally the CLAP loop,
      not apply-at-block-start (unless 0035 measured slicing unnecessary,
      in which case cite that decision).
- [ ] Notes and param changes from the ring take effect at the correct
      sub-block offset (a test asserts onset/param-step sample position).
- [ ] Non-automatable key-mode/split-point applied once per quantum before
      event ingestion, matching the plugin.
- [ ] The host instantiates from wasm bytes in the worklet (no
      wasm-bindgen), reusing the 0034 pattern.
- [ ] No change to `vxn-engine` DSP or `Synth::process`.

## Notes

- Depends on [0037](0037-web-event-codec.md) (decoder) and the 0035 ring;
  proceeds alongside [0039](0039-web-param-store.md) (param reads).
- Hardened by [0040](0040-web-worklet-lifecycle.md) (lifecycle, traps).
- This replaces, in the web world, the per-batch slicing the host gives a
  CLAP plugin for free — see the E015 background section.
- Out of scope: AudioContext bootstrap / build pipeline (E016), input
  sources (E017).

## Close-out (2026-06-14)

- **Rust audio-host** [host.rs](../../vxn-1/crates/vxn-wasm/src/host.rs): a
  `Host` owning the `Synth`, stereo output, and a linear-memory event-decode
  scratch. `vxn_host_render(ptr, n_events, key_mode, split_point)` ports the
  CLAP batch loop (vxn-clap/src/lib.rs:286-390) into ONE wasm call: set
  non-automatable shared state once, then slice the block at each record's
  sample offset (`codec::decode_and_apply` per event), render `[prev..k)`
  between events, render the tail. Engine unchanged — `Synth::process` still
  renders contiguous slices; the host owns slicing. C-ABI is raw
  `WebAssembly.instantiate`, no wasm-bindgen (0034 pattern): `vxn_host_new`/
  `_destroy`/`_events_ptr`/`_max_events`/`_set_param`/`_render`/`_out_l`/
  `_out_r`.
- **Why a Rust loop** (vs the 0035 JS loop): 0035 drove slicing from JS,
  crossing the JS↔wasm boundary O(events+slices)/quantum. This does it in
  one crossing — JS copies the ring's raw wire bytes into the wasm scratch,
  Rust decodes+slices+renders.
- **Shared JS driver** [audio-host.mjs](../../vxn-1/crates/vxn-wasm/web/audio-host.mjs)
  (`AudioHost`), imported by both the worklet and the Node harness: per
  quantum it (1) folds the 0039 store into the engine block-start
  (`applyStoreToEngine`, changed-only — the `LocalParams` analogue), (2)
  copies due ring bytes into the wasm scratch via the new
  `EventRing.drainRawInto` (consumer-side addition to
  [event-ring.mjs](../../vxn-1/crates/vxn-wasm/web/event-ring.mjs), no
  framing change), (3) one `vxn_host_render`, (4) copies output out.
- **Worklet** [vxn-processor-0038.js](../../vxn-1/crates/vxn-wasm/web/vxn-processor-0038.js):
  instantiates from `processorOptions.wasmBytes`, takes ring+store SABs,
  key-mode/split over the port; `process()` is one `host.process()` call.
- **Sample-accuracy + parity verified.** Rust unit tests (`cargo test -p
  vxn-wasm`, 14/14): note-on at offset N → onset N+1 for N∈{0,1,7,31,63,
  100,127}; one-call render byte-identical to a hand-written apply-then-slice
  reference; param-step tail rendered; empty-quantum silence; split-mode
  render. Node harness
  [harness-0038.mjs](../../vxn-1/crates/vxn-wasm/harness-0038.mjs) (all
  checks pass): same onset sweep through the full ring→wasm-decode→slice
  path, **host output byte-identical (max abs diff 0) to the proven 0035 JS
  `renderQuantumSliced`** on a mixed note+param stream, split-mode render,
  and the 0039 store fold (bulk 165-param preset applies with no glitch; a
  store edit reaches the engine). Param changes on the ring take effect at
  their sub-block offset (covered by the parity stream's offset-16 param).
- **Decision recorded:** key-mode/split are passed to `vxn_host_render` as
  args (the `SharedParams` shared-state analogue), applied once before the
  event loop — they are NOT param ids (ADR 0003 §3). The slice loop still
  accepts EV_KEY_MODE/EV_SPLIT_POINT on the ring (codec handles them) for
  flexibility, last-wins.
- Regression: 0035 harness, 0037 codec JS, 0039 param-store JS all still
  green after the `event-ring.mjs` addition. No `index.html` — AudioContext
  bootstrap is E016.
