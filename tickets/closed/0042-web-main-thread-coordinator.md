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

## Close-out (2026-06-15)

- **Coordinator module.** [coordinator.mjs](../../vxn-1/crates/vxn-wasm/web/coordinator.mjs)
  — `class WebHost`: created on the gesture, `start()` makes the `AudioContext`,
  `addModule`s the worklet + fetches the wasm in parallel, constructs the
  `AudioWorkletNode` over the SABs, connects to `destination`, `resume()`s. Boot
  reaches "audio live": worklet posts `ready`, surfaced via `whenReady`/`onReady`
  (harness §1 `coordinator observed worklet ready`).
- **Shared SAB lifecycle (ADR 0009 §2).** Coordinator owns both transport SABs —
  `createRingSAB` (0035 event ring) + `createParamSAB` (0039 165-atomic store) —
  allocated at construction (producer usable pre-`ready`; events buffer in the
  ring per the silence-until-ready contract), passed to the worklet via
  `processorOptions`. Harness §1 asserts `node.runner.ringSab === host.ringSab`
  and same for store (same-identity main↔worklet map). `crossOriginIsolated` is a
  serving concern (0045); the page surfaces it.
- **Wasm hand-off.** `start()` fetches the engine `.wasm` (or takes pre-fetched
  `wasmBytes`) and passes the bytes through `processorOptions` — the 0034 pattern
  `WorkletHostRunner` expects.
- **Transport wiring.** Producer surface (`noteOn`/`noteOff`/`pitchBend`/
  `modWheel`/`sustain`) writes the ring; `setParam`/`setParamsBulk` write the
  store; `pollParamDiffs` reads the audio→main readback. Harness §2 (`main-thread
  note sounded at its offset`, onset 11) and §3 (`applied value echoed back`)
  prove a note + a param from main reach the audio thread.
- **Store seeding.** Added [`vxn_host_get_param`](../../vxn-1/crates/vxn-wasm/src/host.rs)
  to the C-ABI; `start()` snapshots the engine's 165 defaults off a throwaway
  instance and `writeBulk`s them before node construction, so the worklet's
  NaN-seeded first fold doesn't zero every param (fulfils the 0039 "controller
  seeds the store before the worklet starts" contract). Without it the voice was
  silent (caught + fixed during bring-up).
- **Trap.** Worklet `trap` port message surfaced to `onTrap`; harness §4 forces a
  render-thread trap (`trap surfaced to coordinator.onTrap`), marks not-ready, and
  audio recovers (onset 6) over the same SABs.
- **Build + page.** [xtask web](../../vxn-1/xtask/src/main.rs) bundles
  `coordinator.mjs` into `target/web-dist/` and generates an `index.html` that
  boots `WebHost` (Start + Hold-A4). Verified: `node harness-0042.mjs` ALL CHECKS
  PASSED; `cargo test -p vxn-wasm` 16 passed; harness-0040 + param-store +
  event-codec suites green; `cargo xtask web` assembles clean.
- Browser-only criteria (`crossOriginIsolated` true on a real device, audibility
  through a speaker) need 0045's COOP/COEP server for a manual check; the wiring
  is proven headlessly by the fake-context-over-real-runner harness.
