---
id: "0323"
product: vxn-2
title: "vxn-2: one log-domain level accumulator for the operator amplitude chain"
priority: high
created: 2026-08-29
epic: E048
depends: []
---

## Summary

First ticket of [E048](../../epics/open/E048-log-domain-level-pipeline.md), per
[ADR 0010](../../vxn-2/adrs/0010-log-domain-level-pipeline.md).

`OpState::cook` and `Stack::cook_op` currently build `max_amp` by converting each
contributor to a linear multiplier independently and multiplying:

```rust
let max_amp = (level_norm * ks_lvl).min(1.0) * vel;
```

Replace with a single `i32` accumulator in the hardware's post-`<<5` resolution
(1/32 level unit, ~0.0235 dB), converted to linear exactly once. Contributors may
only be added.

## Design

```
units  = scaleoutlevel(op.level)            // table, 0..99 → 0..127
units += ks_level_offset(key, bp, …)        // already returns level units
units  = min(127, units)                    // ceiling, BEFORE velocity
units *= 32                                 // → 1/32-unit resolution
                                            // (velocity joins here — 0324)
units  = max(0, units)                      // floor only
max_amp = exp2(units as f32 / (32.0 * LEVEL_UNITS_PER_OCTAVE))
```

`ks_level_offset` already exists and returns units — it simply stops being
exponentiated by `ks_level_mult`, which is retired along with its call sites.

`scaleoutlevel` is a new table port. It is *not* the same curve as
`eg::level_to_amp`: the latter maps 0..99 to amplitude directly (ADR 0007), the
former maps 0..99 to the 0..127 level domain the accumulator lives in. Both must
agree at the endpoints — `scaleoutlevel(99) = 127` must land on the same
amplitude `level_to_amp(99) = 1.0` does — and a test should assert that they
agree across the range to within the table's own quantisation, since ADR 0007's
calibration is the thing being preserved.

Keep `exp2` for the final conversion (ADR 0010 §2): control-rate, and more
accurate than the hardware's lookup.

## Acceptance criteria

- [ ] `scaleoutlevel` ported as an integer table; asserted exactly against the
      reference on all 100 inputs, and against `level_to_amp` across the range.
- [ ] Both cook sites build one `i32` accumulator; `ks_level_mult` is gone.
- [ ] Clamp order matches hardware: ceiling before velocity's insertion point,
      floor after. The ceiling is `min(127)` on units, not `.min(1.0)` on a
      linear product.
- [ ] Exactly one `exp2` per operator per cook (was three).
- [ ] EG `max_amp` semantics unchanged — `EgState::cook` still takes a linear
      ceiling; only its derivation moves.
- [ ] `vxn-asm-check` clean; `stack_tick_stereo` SIMD count unchanged.
- [ ] No audible change expected from this ticket alone (velocity still enters
      as a multiplier until 0324) — assert bit-stability on a preset render, or
      state the delta and why.

## Notes

The `.min(1.0)` this replaces was correct only by coincidence: it equals
`min(127)` because `level_to_amp(99) = 1.0`. Making the clamp operate on units
is the point, not a refactor.

## Close-out (2026-08-29)

- [level.rs](../../vxn-2/crates/vxn2-dsp/src/level.rs) holds the accumulator:
  `scale_outlevel` (20-entry `LEVELLUT` + `28 + OL` above the knee),
  `op_max_amp` summing level units and converting once via `exp2`.
- `scale_outlevel` asserted exactly against the reference on all 100 inputs
  (`level::tests::scale_outlevel_matches_hardware`). Its relationship to
  ADR 0007 is pinned in both directions:
  `agrees_with_adr_0007_above_the_knee` (exact for OL ≥ 20 — `28 + OL` against
  full scale 127 *is* `2^((OL−99)/8)`, so 0007's 0.75 dB/step was the hardware's
  own arithmetic) and `diverges_below_the_knee_by_the_table`.
- Both cook sites go through one `cook_max_amp`
  ([op.rs](../../vxn-2/crates/vxn2-dsp/src/op.rs)), following the
  `compute_base_hz` precedent. `grep ks_level_mult` → 0 hits: retired.
- Ceiling is `min(127)` on units, before velocity's insertion point; floor
  after. `key_scaling_cannot_boost_past_nominal` pins both the clamp point and
  an exact unclamped boost ratio.
- Three `exp2` per operator per cook became one.
- 45-preset A/B against the previous commit: 39 bit-identical; the three largest
  movers (Bell Jar −119 dB, Draughtsman −126, Ivory Dust −161) are exactly the
  three presets carrying an operator below the OL 20 knee, the other three
  (−198…−176 dB) float reassociation. Nothing audible, as the ticket predicted.
- `vxn-asm-check` clean; `stack_tick_stereo` 196, unchanged.
