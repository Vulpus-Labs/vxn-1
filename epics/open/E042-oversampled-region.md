---
id: E042
product: vxn-2
title: "Oversampled region — extract SpanDelay + OsRegion so 'OS span containing N kernels' is a reusable unit (behaviour-preserving)"
status: open
created: 2026-08-02
---

> vxn-2's filter + dynamics share one 4× oversampled span: `OsSpan` FSM,
> `SpanDelay` constant-latency bridge, shared decimator with a continuity rule
> ("reset only when the span's engaged state flips"), leg-dependent
> interpolator placement (per-stack when Filtered, global-on-summed-mix when
> DynOnly). Today that machinery is ~900 lines of loose fields in
> [engine.rs](../../vxn-2/crates/vxn2-engine/src/engine.rs). This epic extracts
> the **mechanics** (`OsRegion`: decimator + latency bridge + fade countdown +
> OS buffers) into `vxn-core-dsp`, leaving the **policy** (the FSM, the
> interpolators, which kernels run inside) in the engine — so a future synth
> can build an oversampled region without reinventing the clunk-free lifecycle.

## Goal

- `vxn-core-dsp::os_region` holds `SpanDelay` (pure move) and `OsRegion`.
- Decimator-continuity is a unit-tested `OsRegion` invariant, not a comment.
- vxn-2's `advance_os_span` + three render legs drive an `OsRegion`; dynamics
  runs over `region.bus()` after the per-stack accumulate.
- vxn-2 render hash **unchanged** — this is a pure refactor; if the hash
  moves, the refactor is wrong (fix, never recapture).

## Planned tickets

Chain: **0233 → 0234 → (0235 stretch)**.

- [ ] **0233** — SpanDelay move + `OsRegion` type + continuity invariant tests.
- [ ] **0234** — vxn-2 span plumbing rewritten onto `OsRegion` (hash-unchanged).
- [ ] **0235** — (stretch, skippable) vxn-1 `OutputStage` adopts `OsRegion`.

## Acceptance

- vxn-2 baseline hash, filter/dynamics integration tests, note on/off click
  tests, filter-toggle declick all pass unmodified.
- `filter_path` + `stack` criterion benches within noise; asm-check unchanged.
- `OsRegion` doc explains the leg-dependent interpolator contract (region does
  NOT own interpolators) and the `SpanDelay` constant-latency rationale.
