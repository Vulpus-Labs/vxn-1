---
id: "0036"
product: vxn-2
title: "Spike + ADR: controller placement & cross-thread param store"
priority: high
created: 2026-06-14
epic: E015
depends: []
---

## Summary

The second E015 de-risk spike, run in parallel with
[0035](0035-web-sab-event-ring-spike.md). Decide the two architecture
forks that shape the entire web port, and record them in an ADR:

1. **Controller placement** — compile `vxn-app` + `vxn-core-app` to a
   main-thread wasm and reuse the existing MVC controller verbatim, or
   reimplement the controller in JS and keep wasm to the engine only?
2. **Cross-thread param store** — a `SharedArrayBuffer`-backed atomic
   array shared by both threads (closest to today's `SharedParams`,
   [vxn-clap/src/lib.rs:193-236](../../vxn-1/crates/vxn-clap/src/lib.rs#L193-L236)),
   or param changes carried as ordinary events on the 0035 ring?

These gate the scaffolds (0037-0039) and ripple into E016 (host shell)
and E018 (UI bridge).

## Design

Two small spikes feeding one ADR:

- **Controller-in-wasm probe**: compile `vxn-app` + `vxn-core-app` to
  `wasm32-unknown-unknown` (like 0034 did for the engine). Confirm it
  builds, gauge size, and sketch how `UiEvent`/`ViewEvent`
  ([vxn-core-app events](../../crates/vxn-core-app/src/events.rs)) marshal
  across the JS boundary. The win is reuse + MVC discipline
  ([[vxn2-mvc-discipline]]); the cost is a second wasm module and JS↔wasm
  marshalling. Note the `Arc<Mutex<Controller>>` + bounded channels model
  collapses to single-threaded on the main thread (Mutex → `RefCell`).
- **Param-store probe**: prototype both — (a) a `SharedArrayBuffer` of 165
  atomics indexed by CLAP id, read lock-free by the worklet, written by
  the controller; (b) param-set events on the 0035 ring. Stress the
  bulk-preset-load case (165 params at once) and the audio→main diff
  readback (the param-diff pump's job: detect audio-thread writes, emit
  `ParamChanged`). Two wasm memories don't share by default — if the
  controller is a separate wasm, option (a) needs a shared-memory build or
  a dedicated SAB the worklet also maps.

Deliver an ADR (in `adrs/`) capturing: the decision, the rejected
alternative, and the consequences for 0037/0038/0039 and E016/E018.

## Acceptance criteria

- [ ] `vxn-app` + `vxn-core-app` compile to `wasm32-unknown-unknown` (or
      the blocker is documented), with a size/feasibility note.
- [ ] Both param-store options prototyped far enough to compare on
      bulk-load latency and the diff-readback path.
- [ ] An ADR records controller placement + param-store mechanism, with
      rationale and downstream consequences.
- [ ] The ADR names the concrete param addressing (CLAP-id layout: 69×2
      patch + 27 global = 165) the codec (0037) and store (0039) will use.

## Notes

- Pairs with [0035](0035-web-sab-event-ring-spike.md). Together the two
  spikes fully determine the E015 scaffolds.
- Param model reference: `vxn-app/src/params.rs` (definitions),
  `vxn-engine/src/shared.rs` (`SharedParams`), `vxn-engine/src/params.rs`
  (storage). Related: [[vxn1-id-stability-dropped]] (param ids need not be
  append-only — clean addressing is fine).
- Out of scope: implementing the chosen store (0039) or controller shell
  (E016) — this ticket only decides and records.
