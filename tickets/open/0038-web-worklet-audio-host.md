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
