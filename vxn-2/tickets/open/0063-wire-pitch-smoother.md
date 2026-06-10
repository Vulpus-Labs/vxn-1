---
id: "0063"
title: "Instantiate PitchSmoother in Engine::process_block"
priority: high
created: 2026-06-10
epic: E006
depends: []
---

## Summary

Third ticket of [E006](../../epics/open/E006-review-remediation.md).
`PitchSmoother` is fully implemented and unit-tested
([matrix.rs:610-671](../../crates/vxn2-engine/src/matrix.rs#L610)), and
the matrix module doc
([matrix.rs:31-36](../../crates/vxn2-engine/src/matrix.rs#L31)) promises
that pitch-shaped destinations "get one-pole smoothing from the block
accumulator down to per-sample" — but `Engine::process_block` never
constructs one. Pitch offsets are written straight from the block-rate
`dest_vals` buffer into `stack.op_pitch_mod_st` /
`stack.global_pitch_mod_st`, so LFO1 → GlobalPitch steps once per
control block: audible zipper at block sizes ≥ 64.

Wire the existing struct in; no new smoothing design needed.

## Where

- `engine.rs` — add a `PitchSmoother` per stack (or per pitch-shaped
  dest slot, matching the canonical order at
  [matrix.rs:371](../../crates/vxn2-engine/src/matrix.rs#L371)) to
  `Engine` state. Per block: `targets_from` the dest accumulator,
  `tick` toward targets, feed the smoothed values to the stack instead
  of the raw block values. Use `snap_to` on note-on / reset so a fresh
  voice doesn't glide in from the previous voice's offset.
- Decide and document the granularity: true per-sample smoothing means
  the stack pitch fields update inside the sample loop (cost: 48
  `powf`-equivalent pitch recooks per sample is NOT acceptable — check
  how `apply_pitch_mult` cost scales). If per-sample recook is too
  hot, smooth at a sub-block quantum (e.g. every 8–16 samples) and
  document the compromise in the module doc. The acceptance bar is
  "no audible stepping", not a particular implementation.

## Acceptance criteria

- [ ] Engine integration test: route LFO1 (slow sine) → GlobalPitch at
  full depth, render at block size 256, take the per-sample pitch
  trajectory (or output zero-crossing intervals) and assert no
  discontinuity larger than the equivalent of a few cents between
  adjacent samples — i.e. the staircase is gone.
- [ ] `snap_to` on note-on verified: first block of a fresh note has
  no glide-in from a stale smoother state (test).
- [ ] `cargo bench --package vxn2-osc-bench master_chain` regression
  ≤ 5%.
- [ ] Module doc at matrix.rs:31 matches what actually ships
  (per-sample vs sub-block quantum).
- [ ] `PitchSmoother` no longer flagged by `cargo +nightly udeps`-style
  dead-code inspection — it has a production call site.

## Notes

The review classified the unwired smoother as the third "last wire
not connected" instance alongside lfo1-depth (0061) and AmpSens
(0062). The `DestId::*.idx().unwrap()` constants computed at the top
of `process_block`
([engine.rs:277-284](../../crates/vxn2-engine/src/engine.rs#L277))
should become module-level `const`s while you're in the function —
review nit, zero-cost, kills live `unwrap()`s in the hot path.
