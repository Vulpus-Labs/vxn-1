---
id: E015
product: vxn-2
title: "vxn-1 web port — event-driven core (audio-thread event model)"
status: open
created: 2026-06-14
depends-on: null
---

> **First and highest-risk epic of the vxn-1 browser/WASM port.**
> Feasibility is proven — spike [0034](../../tickets/closed/0034-vxn1-wasm-spike.md)
> compiled `vxn-engine` to `wasm32-unknown-unknown` with zero source
> changes and rendered audio inside an `AudioWorkletProcessor`. What the
> spike did *not* exercise is the hard part: replicating the CLAP
> sample-accurate, lock-free event model across the
> **main-thread ↔ AudioWorklet** boundary. This epic foregrounds exactly
> that, before any UI, input, or packaging work. The transport and
> threading decisions made here are load-bearing for E016–E019.

## Goal

Stand up the audio-thread event core of the web synth: a lock-free,
(near) sample-accurate path for delivering parameter changes, gestures,
and note/MIDI events from the main thread into the wasm engine running in
an `AudioWorkletProcessor`, plus the readback path for audio-thread state
(automation echo, meters) — mirroring what `vxn-clap` does today, but
over a `SharedArrayBuffer` instead of the CLAP host ABI.

When this epic closes:

- A `SharedArrayBuffer` ring buffer carries a compact binary event stream
  from the main thread to the worklet, drained every render quantum.
- The worklet host **slices the render block at event sample-offsets**,
  applying events then rendering each sub-slice — a direct port of the
  CLAP batch loop, so timing parity with the plugin is structural, not
  approximate.
- Parameters cross threads lock-free (the `SharedParams` analogue), and
  audio-thread param drift (host-automation-style writes) is observable
  from the main thread — the param-diff pump, ported.
- The controller-placement question (main-thread Rust-wasm vs JS) is
  decided and recorded in an ADR.
- Cross-origin isolation (COOP/COEP) is proven to enable `SharedArrayBuffer`
  in the target browsers, with a documented serving story.

## Why the event core first

The engine is a solved problem (0034). The risk that could sink the whole
port lives entirely in the boundary:

- **AudioWorklet is a separate realtime thread.** Unlike the CLAP plugin —
  where the host hands `process()` a sample-accurate event list on the
  audio thread itself — the browser splits the controller (main thread)
  from the renderer (worklet thread). Crossing that gap glitch-free and
  with predictable timing is the novel engineering.
- **`postMessage` is jittery and non-sample-accurate.** It's fine for a
  spike button, wrong for musical timing. The real answer is a
  `SharedArrayBuffer` ring buffer with timestamps — which drags in
  `Atomics`, cross-origin isolation (COOP/COEP) headers, and lifecycle
  subtleties. Better to hit those on day one than after a UI exists.
- **Everything downstream assumes this contract.** Input (E017) writes
  events into the ring; the UI bridge (E018) writes param/gesture events
  and reads back ViewEvents; persistence (E019) bulk-applies params on
  preset load. If the transport shape is wrong, all three rework.

## Background — what we are replicating

The current plugin's audio-thread loop (`VxnAudioProcessor::process`,
[vxn-clap/src/lib.rs:286-390](../../vxn-1/crates/vxn-clap/src/lib.rs#L286-L390))
is the reference design:

```text
synth.set_key_mode(shared.key_mode());      // non-automatable state, once per block
synth.set_split_point(shared.split_point());
for event_batch in events.input.batch() {   // host-provided sample-accurate batches
    for event in event_batch.events() {
        ParamValue -> synth.set_param(idx, value)
        Note/MIDI  -> dispatch_event(SynthNotes(synth), ...)  // vxn-core-clap events.rs:43-89
    }
    let (start, end) = event_batch.sample_bounds();
    synth.process(&mut l[start..end], &mut r[start..end]);    // render the sub-slice
}
```

Key facts the web host must honour (from the event-flow survey):

- **`Synth::process` does NOT support mid-block events.** Sample accuracy
  is achieved *entirely by the shell* slicing the block and calling
  `process()` per slice. The web host owns that slicing — the engine
  needs no change.
- **Params are lock-free via `SharedParams`** (atomics): audio reads, main
  writes. A timer-tick **param-diff pump**
  ([lib.rs:193-236](../../vxn-1/crates/vxn-clap/src/lib.rs#L193-L236))
  detects audio-thread writes and emits `ViewEvent::ParamChanged` so the
  UI sees automation the controller never processed.
- **165 CLAP param ids** (69 per-patch × 2 layers + 27 global) — the store
  must address all of them compactly.
- **Key mode / split are non-automatable shared state**, set once per
  block before event ingestion, updated via a `UiEvent::Custom` payload —
  not a param. The ring/store design must carry these too.

## Scope

**In:**

- A `SharedArrayBuffer` **event ring buffer** (SPSC: main writes, worklet
  reads) with timestamped, length-prefixed binary records.
- A **binary event codec** — param-set, param-set-norm, gesture
  begin/end, note-on/off, pitch-bend, mod-wheel, sustain, key-mode,
  split-point — encoded by the main thread, decoded by the worklet,
  mirroring `vxn-core-clap` `dispatch_event` semantics. Rust + JS
  implementations with round-trip agreement tests.
- A **worklet audio-host** (wasm, in the worklet) that ports the CLAP
  batch loop: drain ring → apply events → slice block at offsets →
  `Synth::process` per slice. Replaces `vxn-clap`'s `process()`.
- A **cross-thread param store** (the `SharedParams` analogue) and the
  **audio→main param-diff readback** for automation echo / meters.
- An ADR deciding **controller placement** (main-thread Rust-wasm reusing
  `vxn-app`/`vxn-core-app` vs a JS reimplementation) and where the param
  store lives (SAB-backed shared atomics vs param-events-on-the-ring).
- Worklet **lifecycle**: instantiate-from-bytes, silence-until-ready,
  suspend/resume, sample-rate from the `sampleRate` global, teardown,
  and render-thread trap safety (a wasm panic must not wedge audio).
- A COOP/COEP dev-serving story sufficient to enable `SharedArrayBuffer`.

**Out:**

- The AudioContext bootstrap, build/bundle pipeline, production hosting
  → **E016**.
- Web MIDI / keyboard input sources → **E017** (they *write into* this
  epic's ring, but the input plumbing is separate).
- The HTML faceplate and the JS↔controller opcode bridge → **E018**.
- Preset / state persistence → **E019**.
- Perf tuning, SIMD128, cross-browser matrix → **E020**.
- Any change to `vxn-engine` DSP or the param table.

## Tickets

- [ ] [0035 — Spike: SAB event-ring transport + worklet block-slicing](../../tickets/open/0035-web-sab-event-ring-spike.md)
- [ ] [0036 — Spike + ADR: controller placement & cross-thread param store](../../tickets/open/0036-web-controller-placement-adr.md)
- [ ] [0037 — Scaffold: binary event codec (Rust + JS, round-trip tested)](../../tickets/open/0037-web-event-codec.md)
- [ ] [0038 — Scaffold: worklet audio-host (port the CLAP batch loop)](../../tickets/open/0038-web-worklet-audio-host.md)
- [ ] [0039 — Scaffold: cross-thread param store + audio→main diff readback](../../tickets/open/0039-web-param-store.md)
- [ ] [0040 — Harden: worklet lifecycle, sample-rate, trap safety](../../tickets/open/0040-web-worklet-lifecycle.md)

## Dependency order

```text
0035 (transport spike) ─┐
                        ├─> 0037 (codec) ─> 0038 (audio-host) ─┐
0036 (placement ADR) ───┘                                     ├─> 0040 (lifecycle harden)
        │                                                     │
        └────────────────> 0039 (param store) ───────────────┘
```

- **0035 and 0036 run first and in parallel** — they are the two spikes
  that decide the architecture. 0035 proves the ring + slicing mechanism
  and the COOP/COEP requirement; 0036 decides where the controller and
  param store live.
- 0037 (codec) needs 0035's record framing and 0036's param addressing.
- 0038 (audio-host) consumes the codec and the ring; it is the scaffold
  that makes the spike permanent.
- 0039 (param store) implements 0036's decision; it can proceed alongside
  0038.
- 0040 hardens lifecycle once a render loop exists end-to-end.

## Open questions — resolved by the spikes

- **Controller placement** (0036): compile `vxn-app` + `vxn-core-app` to a
  main-thread wasm and reuse the existing MVC controller verbatim, or
  reimplement the controller in JS and keep wasm to the engine only?
  Trade-off: Rust reuse + MVC discipline (`vxn2-mvc-discipline`) vs JS
  simplicity and one less wasm module.
- **Param store mechanism** (0036): a `SharedArrayBuffer`-backed atomic
  array shared by both threads (closest to today's `SharedParams`), or
  param changes as ordinary events on the ring (simpler, but loses the
  "latest value wins / lock-free read" property the audio thread relies
  on). Bulk preset loads (165 params at once) stress this choice.
- **Sample-accuracy fidelity** (0035): full per-event block slicing
  (timing parity with the plugin, CPU cost per event) vs apply-at-block-
  start (one-quantum latency, simpler). Decide per measured jitter.

## Risks

- **`SharedArrayBuffer` requires cross-origin isolation.** COOP/COEP
  response headers are mandatory in all modern browsers; they constrain
  hosting (no third-party scripts without CORP) and complicate local dev.
  If isolation proves impractical for the deploy target, fall back to
  `postMessage` with a measured latency budget — 0035 must quantify this.
- **Atomics in the AudioWorklet.** `Atomics.wait` is forbidden on the
  render thread; the worklet must spin-free-poll the ring. Getting an
  SPSC ring correct (memory ordering, wrap-around, overflow policy) is
  classic lock-free territory — easy to get subtly wrong.
- **Two wasm memories don't share by default.** If the controller runs as
  a second main-thread wasm, it cannot see the engine's linear memory.
  Either share one `SharedArrayBuffer`-backed memory (needs the
  shared-memory/atomics build features and `Synth` audited for it) or
  pass everything through the ring/store. 0036 must pick.
- **Safari worklet + SAB quirks.** Historically the laggard for
  AudioWorklet, Atomics, and isolation behaviour — keep it in view even
  though the cross-browser matrix is E020.
- **Render-thread panics.** A wasm trap in `process()` can permanently
  silence the context. Need a trap boundary that recovers or fails loud.

## Acceptance

- A `SharedArrayBuffer` event ring delivers note and param events from the
  main thread into the worklet, drained every quantum, with no audible
  glitches under a sustained event stream.
- The worklet host slices the render block at event offsets and renders
  per slice — verified by a test showing a note-on at a known sub-block
  offset takes effect at that offset (timing parity with the CLAP loop).
- Parameters cross threads lock-free; an audio-thread param write is
  observable from the main thread via the diff readback.
- An ADR records the controller-placement and param-store decisions with
  rationale.
- COOP/COEP serving enables `SharedArrayBuffer` locally, with the
  fallback position documented if isolation is unavailable.
- No change to `vxn-engine` DSP or the param table.
