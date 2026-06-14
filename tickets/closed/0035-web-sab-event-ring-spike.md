---
id: "0035"
product: vxn-2
title: "Spike: SAB event-ring transport + worklet block-slicing"
priority: high
created: 2026-06-14
epic: E015
depends: []
---

## Summary

First and most important de-risk of the vxn-1 web port — the foreground
spike of [E015](../../epics/open/E015-web-event-driven-core.md). Prove a
lock-free, (near) sample-accurate path from the main thread into the
`AudioWorkletProcessor` over a `SharedArrayBuffer`, and prove the worklet
can **slice its render block at event sample-offsets** the way the CLAP
shell does today
([vxn-clap/src/lib.rs:335-369](../../vxn-1/crates/vxn-clap/src/lib.rs#L335-L369)).

Builds directly on spike [0034](../../tickets/closed/0034-vxn1-wasm-spike.md),
which proved the engine renders in a worklet but drove notes via
`postMessage` (jittery, not sample-accurate). This spike replaces that
with the real transport.

Throwaway exploration — extends the `vxn-wasm` spike crate / `web/`
harness. Outcome is a go/no-go on the mechanism plus the decisions feeding
0037/0038, not shippable code.

## Design

- **SPSC ring buffer in a `SharedArrayBuffer`**: main thread writes,
  worklet reads. Length-prefixed binary records, each carrying a sample
  timestamp (offset within the upcoming render quantum). Lock-free via
  `Atomics` on read/write indices. **No `Atomics.wait` on the render
  thread** — the worklet free-polls in `process()`.
- **Block-slicing in the worklet**: each `process()` call, drain all
  records due this quantum, and mirror the CLAP batch loop — apply events
  at offset `k`, render `[prev..k)`, repeat, render the tail. The engine
  stays unchanged (`Synth::process` renders contiguous slices; the host
  owns slicing).
- **COOP/COEP**: serve the harness with
  `Cross-Origin-Opener-Policy: same-origin` +
  `Cross-Origin-Embedder-Policy: require-corp` and confirm
  `crossOriginIsolated === true` and `SharedArrayBuffer` is constructible.
- **Measurement**: drive a metronomic note stream at known sub-block
  offsets; verify notes take effect at the right offset (compare rendered
  onset sample vs intended). Compare jitter against the 0034
  `postMessage` path. Stress with a dense param+note stream; confirm no
  xruns and define a ring-overflow policy (drop-oldest vs block-writer).

Decide and record for downstream tickets:
- Record framing (header layout, max record size, ring capacity).
- Full per-event slicing vs apply-at-block-start, by measured jitter
  (E015 open question).
- Whether `SharedArrayBuffer` isolation is viable for the deploy target,
  or a `postMessage` fallback budget is needed.

## Acceptance criteria

- [ ] A `SharedArrayBuffer` SPSC ring delivers binary event records from
      the main thread to the worklet, drained every quantum, no
      `Atomics.wait` on the render thread.
- [ ] The worklet slices the render block at event offsets: a note-on
      written for sub-block offset N takes audible/measurable effect at
      offset N (not at block start) — timing parity with the CLAP loop.
- [ ] `crossOriginIsolated === true` on the served harness; documented
      COOP/COEP headers reproduce it.
- [ ] Jitter measured vs the 0034 `postMessage` path; ring-overflow policy
      chosen and noted.
- [ ] Short writeup: framing decisions, slicing-fidelity decision, and the
      isolation go/no-go (with fallback if no-go) — feeding 0037/0038.

## Notes

- Pairs with [0036](0036-web-controller-placement-adr.md) (the other
  E015 spike) — run in parallel. 0036 decides *where the controller and
  param store live*; this ticket decides *how events cross the thread
  boundary*.
- Related: [[vxn2-mvc-discipline]] (event-driven discipline), the CLAP
  reference loop, `vxn-core-clap` `dispatch_event`.
- Out of scope: the binary codec proper (0037), the permanent audio-host
  (0038), input sources (E017).

## Close-out (2026-06-14)

- **SPSC SAB ring + drain.** Lock-free ring over a `SharedArrayBuffer`
  (monotonic slot counters masked to a power-of-two index, `Atomics`
  release-store-after-write / acquire-load-before-read, **no `Atomics.wait`
  on the render thread**) in
  [event-ring.mjs](../../vxn-1/crates/vxn-wasm/web/event-ring.mjs), imported
  verbatim by both the worklet
  ([vxn-processor-0035.js](../../vxn-1/crates/vxn-wasm/web/vxn-processor-0035.js))
  and the Node harness — one code path. Framing: fixed **16-byte slots**,
  capacity **1024 slots = 16 KiB** (~8 quanta headroom).
- **Worklet block-slicing = CLAP parity.** Added
  `vxn_process_slice(ptr,start,end)` + `vxn_set_param(ptr,idx,value)` to
  [lib.rs](../../vxn-1/crates/vxn-wasm/src/lib.rs) (existing exports kept,
  engine untouched). Drain → apply-at-offset → render sub-slice mirrors
  [vxn-clap/src/lib.rs:335-369](../../vxn-1/crates/vxn-clap/src/lib.rs#L335-L369).
  Harness proves **0-frame onset error at every offset 0…127** — exact
  sample accuracy.
- **Jitter measured vs block-start (postMessage-equiv):** sliced 0 µs vs
  block-start **up to 127 frames = 2645.8 µs** @ 48 kHz. Slicing-fidelity
  decision: **full per-event slicing** (the gap justifies the negligible
  CPU cost). Overflow policy: **block-writer, never drop** (musical events
  must not be lost). Dense-stream stress: 48 000 events, 0 dropped / 0
  reordered; forced overflow refuses then recovers.
- **Isolation: GO.** COOP `same-origin` + COEP `require-corp` (+ CORP
  `same-origin`) via [serve-coep.mjs](../../vxn-1/crates/vxn-wasm/serve-coep.mjs)
  flips `crossOriginIsolated` to `true` and makes `SharedArrayBuffer`
  constructible. Fallback if ever unavailable: `postMessage` block-start,
  budgeted ~2.65 ms onset jitter.
- **Writeup:** [SPIKE-0035-findings.md](../../vxn-1/crates/vxn-wasm/SPIKE-0035-findings.md)
  records framing / slicing / overflow / isolation decisions feeding
  0037/0038. Headless Node proof
  ([harness-0035.mjs](../../vxn-1/crates/vxn-wasm/harness-0035.mjs)): all
  checks pass, ~50× realtime at 8 sliced events/quantum. Browser audibility
  ([index-0035.html](../../vxn-1/crates/vxn-wasm/web/index-0035.html)) is a
  manual check (this env is headless).
