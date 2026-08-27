---
id: "0239"
product: monorepo
title: "Runtime-swap readiness — documented block-edge kernel-selection pattern + KernelSelectFn + compile-tested example"
priority: low
created: 2026-08-02
epic: E044
depends: ["0238"]
---

## Summary

Final ticket of [E044](../../epics/open/E044-envelope-lifecycle-swap-readiness.md).
Locked decision: design for runtime swapping of oscillators / envelope curves /
filter implementations, wire nothing user-facing yet. The repo already has
both safe dispatch shapes; this ticket canonises them in vxn-core-dsp docs so
a future swap param has a recipe that provably keeps dispatch out of lane
loops:

- **Marker monomorphisation**: runtime enum resolved once outside the lane
  loop via a `with_X!` macro to a ZST marker generic
  (`WaveKind`/`with_wave!` at
  [poly/oscillator.rs:114-177](../../vxn-1b/crates/vxn-dsp/src/poly/oscillator.rs#L114-L177),
  `LadderMix`/`with_mix!`). Shared as a documented macro template — the
  markers themselves stay per-synth (moving them couples codegen for zero
  dedup).
- **Fn-ptr table**: per-block function pointer stored on voice state,
  resolved at block/note edge (`LANE_ROUTE_FNS`, 32 `#[inline(never)]`
  symbols at
  [stack.rs:112-180](../../vxn-2/crates/vxn2-dsp/src/stack.rs#L112-L180)) —
  the deliberate anti-monomorphisation choice for high-cardinality selection.

## Acceptance criteria

- [ ] vxn-core-dsp doc page: when to use which shape, the block-edge
      resolution rule, the worst-case-sized-state rule (fat `EgState`
      per-lane, no boxing), the enum-match-in-loop ban with the measured
      evidence pointers ([[vxn1-soa-match-defeats-simd]], the ~50% nyquist-
      fade branch regression note at `stack.rs:264-268`).
- [ ] `pub type KernelSelectFn<...>` alias + one compile-tested example
      (doc-test or tests/) swapping between two toy kernels at a block edge.
- [ ] No user-facing params added; no engine rewiring.

## Notes

This closes the extraction epics. A future "filter model" / "env curve" param
is then: add enum + table/macro arm, resolve at block edge, state already
sized — additive work, no re-architecture.
