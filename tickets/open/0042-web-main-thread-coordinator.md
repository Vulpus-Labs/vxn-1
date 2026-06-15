---
id: "0042"
product: vxn-2
title: "Main-thread coordinator: AudioContext + worklet bootstrap over E015 transport"
priority: high
created: 2026-06-15
epic: E016
depends: ["0041"]
---

## Summary

The main-thread coordinator module: create the AudioContext, add the worklet
module, instantiate the `AudioWorkletNode`, allocate the shared SABs, hand the
worklet its wasm bytes, and wire it to the E015 transport — taking the served
page to the "audio live" state. The web analogue of the `vxn-clap` host's
audio-side bootstrap.

## Design

- **Coordinator module** (`web/coordinator.mjs` or similar): a single
  `class WebHost` the page constructs on the autoplay-unlock gesture (lifecycle
  detail is 0043; this ticket gets to first sound).
- **AudioContext + worklet.** `new AudioContext()`, `audioWorklet.addModule(...)`
  the E015 [vxn-processor-0038.js](../../vxn-1/crates/vxn-wasm/web/vxn-processor-0038.js),
  construct the `AudioWorkletNode`, connect to `destination`.
- **Shared SAB lifecycle (ADR 0009 §2).** Own the two transport SABs: the 0035
  event ring and the 0039 param store (165 `Int32` atomics). Allocate them on
  the main thread, pass both to the worklet via `processorOptions` so the
  `WorkletHostRunner` ([host-runner.mjs](../../vxn-1/crates/vxn-wasm/web/host-runner.mjs))
  maps them — the existing runner already accepts `ringSab`/`storeSab`.
- **Wasm bytes hand-off.** Fetch the engine `.wasm` from the dist/ and pass the
  bytes via `processorOptions` (the 0034 pattern the runner expects).
- **Transport wiring.** Main writes note/param events into the ring (the
  producer side of [event-ring.mjs](../../vxn-1/crates/vxn-wasm/web/event-ring.mjs))
  and param values into the store; the worklet consumes. Surface the worklet's
  `ready`/`trap` port messages. A test note from the main thread must sound.

## Acceptance criteria

- [ ] Constructing the coordinator on a user gesture boots an AudioContext,
      loads the worklet, and reaches "audio live" (worklet posts `ready`).
- [ ] The event ring + param-store SABs are allocated on main and shared into
      the worklet; `crossOriginIsolated` is true on the served page.
- [ ] A note event written to the ring from the main thread sounds; a param
      written to the store takes effect on the audio thread.
- [ ] A worklet `trap` is surfaced to the coordinator (recovery policy is the
      runner's; the coordinator just observes + can re-init).

## Notes

- Depends on [0041](0041-web-xtask-build-bundle.md) for the dist/ it loads from.
- Reuses E015 wholesale: ring/store/host-runner are the contract; this ticket
  is the main-thread half that instantiates and feeds them.
- Out of scope: autoplay/suspend/resume/devicechange/teardown (0043); the
  controller wasm + UiEvent marshalling (0044); COOP/COEP serving (0045).
