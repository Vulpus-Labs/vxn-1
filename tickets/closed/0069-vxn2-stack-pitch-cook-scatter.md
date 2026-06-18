---
id: "0069"
product: vxn-2
title: "Cook-time scatter of stack-pitch across the component + re-cook triggers"
priority: medium
created: 2026-06-18
epic: E022
depends: ["0067", "0068"]
---

## Summary

Make the stack-pitch dests actually bend pitch. At cook time, resolve each
`OpNStackPitch` target to its component (ticket 0067) and **scatter** the
route's modulated pitch value — same semitone delta — into the per-op pitch
of every op in that component. Re-resolve when the algo or any op's
Ratio/Fixed mode changes.

## Design

- **Wall mask from freq-mode.** Build a 6-bit `wall_mask` where bit i is set
  iff op (i+1)'s `opN-ratio-mode` is `RatioMode::Fixed`
  ([op.rs](../../vxn-2/crates/vxn2-dsp/src/op.rs),
  [shared.rs](../../vxn-2/crates/vxn2-engine/src/shared.rs#L31)). Call
  `pitch_stack_component(algo, wall_mask, target)` per active stack-pitch
  route.
- **Scatter, don't duplicate eval.** The matrix eval writes each
  `OpNStackPitch` accumulator as today (one lane per target op, 0068). After
  eval, a scatter pass adds each stack-pitch accumulator value into the
  per-op pitch (`OpMPitch` path / `phase_inc` cook) for every op M in
  `component(N)`. Equal value to each member — **no depth scaling**
  (ratio-lock requires identical delta).
  - Per-lane: stack-pitch is a per-lane dest, so the scatter runs in the
    same per-lane pitch cook that already consumes `OpNPitch`.
- **Precompute component masks.** `component(N)` depends only on
  `(algo, wall_mask)` — compute all six masks once per cook, cache, reuse for
  every lane/sample. They are 6 bytes; recompute on the re-cook triggers
  below, not per block.
- **Re-cook triggers** (ADR 0003 dirty-bitset pump): add `algo` and the six
  `opN-ratio-mode` enums as trigger sources for the stack-pitch mask cache.
  - **Ratio↔Fixed toggle only** — a ratio *value* change (2.0→3.0) keeps the
    op tracking, component unchanged, must NOT re-resolve. Gate on the mode
    enum bit, not the continuous ratio param.
  - Algo change already triggers a stack re-cook
    ([engine.rs:462 `set_algo_live`](../../vxn-2/crates/vxn2-engine/src/engine.rs#L462));
    fold the mask recompute into that path.
- **Target-op fixed** → component empty (0067) → scatter adds nothing →
  clean no-op.

## Acceptance criteria

- [ ] Stack-pitch route bends op N + its whole component by an identical
      semitone delta; per-op pitch routes still work unchanged (additive).
- [ ] Ratio-lock verified: under a static stack-pitch offset on a harmonic
      patch, every op's frequency scales by the same factor (FM ratios
      constant) — assert in a unit/integration test.
- [ ] Fixed op excluded and acts as a wall (a fixed mid-chain op splits the
      bend into two independent components).
- [ ] Component masks recompute on algo change and on any Ratio↔Fixed
      toggle; a ratio-value tweak does **not** re-resolve (assert no recook).
- [ ] No per-block / per-sample allocation; masks cached across the cook.
- [ ] Audibility: a non-trivial stack-pitch depth produces a measurable
      pitch shift across the branch (no silent route).

## Notes

- Keep the scatter in the cook, not the audio inner loop — it only changes
  the per-op pitch increment that the SoA op tick already reads, so SIMD
  packing ([[vxn2-stack-soa]]) is untouched.
- Shared-modulator components can be large by design — that is correct
  ratio-lock behaviour, not a bug; document in 0071.

## Close-out (2026-06-18)

- Cook-time scatter in [engine.rs](../../vxn-2/crates/vxn2-engine/src/engine.rs):
  `scatter_stack_pitch` folds each `OpNStackPitch` accumulator (equal semitone
  delta, no depth scaling) into the per-op pitch columns of every op in
  `component(N)`, run per stack right after `eval_dests` and *before* the
  `PitchSmoother` captures targets — so existing smoothing + `project_pitch_state`
  + `apply_pitch_mult` carry it with no audio-inner-loop change.
- Wall mask built from `voice.ops[op].ratio_mode == Fixed`; masks cached on the
  `(algo, wall_mask)` key in `recompute_stack_pitch_masks`, folded into
  `apply_block_params` beside `set_algo_live`. Ratio-value tweak keeps the key →
  no re-resolve; Ratio↔Fixed toggle or algo change re-cooks
  (`stack_pitch_masks_recook_gating`).
- Ratio-lock asserted at the frequency level: all branch ops' `phase_inc` scale
  by one common factor (`stack_pitch_ratio_lock_preserves_fm_ratios`). Equal
  delta + additivity with per-op pitch (`stack_pitch_scatters_equal_delta_across_component`,
  `stack_pitch_additive_with_per_op_pitch`); wall split
  (`stack_pitch_wall_splits_component`); fixed-target no-op
  (`stack_pitch_fixed_target_is_noop`).
- No per-block/per-sample allocation (masks are 6 cached bytes); un-targeted
  path gated by `stack_pitch_targeted()` → bit-identical to pre-E022.
