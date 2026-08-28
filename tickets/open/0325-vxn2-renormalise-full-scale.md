---
id: "0325"
product: vxn-2
title: "vxn-2: renormalise full scale so the velocity boost has headroom"
priority: high
created: 2026-08-29
epic: E048
depends: ["0324"]
---

## Summary

Ticket of [E048](../../epics/open/E048-log-domain-level-pipeline.md), per
[ADR 0010 §4](../../vxn-2/adrs/0010-log-domain-level-pipeline.md).

0324 lets velocity push an operator above its nominal level, as the hardware
does. VXN2 cannot currently represent that. `Engine::cook_stacks_block` stage 8
computes:

```rust
let eff = (eg * (1.0 + level_targets[op_i][k])).clamp(0.0, 1.0);
```

and that single bound is load-bearing: its comment states it is "the ONE bound
for the whole path", and that because both the ramp's start and end points are
in range, the per-sample interpolation is too — *"so `stack_tick_*` needs no
per-sample clamp"*. An `eg` above `1.0` is silently truncated and the boost is
lost.

## Design

Renormalise rather than raise the ceiling. Define amplitude `1.0` as the maximum
**attainable** level — nominal plus the maximum velocity boost, a factor of 1.83
(+5.25 dB) — so a nominal `OL 99` carrier sits at `0.546`.

```
max_amp = exp2((units - UNITS_AT_FULL_SCALE) / (32.0 * LEVEL_UNITS_PER_OCTAVE))
```

where `UNITS_AT_FULL_SCALE = 127*32 + vel_level_offset(127, 7)`.

The `[0, 1]` invariant holds untouched, stage 8 is unchanged, and the hot loop
never learns about any of it. Cost is ~5.25 dB of output across the board,
absorbed by 0326's re-sweep.

Rejected: raising the clamp to ~1.85. It re-opens the ramp-range invariant that
stage 8's comment depends on, and would need the per-sample clamp that loop was
designed to avoid.

## Acceptance criteria

- [ ] Full-scale constant derived from the tables, not written as a literal —
      if `ScaleVelocity`'s ladder changes, the normalisation follows.
- [ ] A `kvs 7` operator at velocity 127 reaches `max_amp` ≈ 1.0 and is *not*
      clamped; assert the rendered level, not just the cooked value.
- [ ] Stage 8's `clamp(0.0, 1.0)` is unchanged and still the only bound.
- [ ] `vxn-asm-check` clean; `stack_tick_stereo` SIMD count unchanged.
- [ ] Bank output drops ≈5.25 dB uniformly — measured, and confirmed uniform
      (a non-uniform drop means a contributor is still outside the accumulator).

## Notes

The uniformity check is the real test of E048: if every contributor is genuinely
inside one accumulator, renormalising it moves everything by the same amount.
