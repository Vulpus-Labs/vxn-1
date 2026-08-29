---
id: "0325"
product: vxn-2
title: "vxn-2: give the velocity boost headroom above nominal"
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

Raise the stage-8 ceiling from `1.0` to `level::MAX_ATTAINABLE_AMP` (≈1.834 —
nominal plus the largest boost the velocity ladder can produce, derived from the
ladder rather than written as a literal). The invariant the lane loop relies on
is *bounded at both ends*, not *bounded at 1.0*, so a different constant leaves
it intact.

**Rejected — and this ticket is where it was rejected.** The original plan was to
renormalise so the maximum attainable level *is* 1.0, putting nominal at 0.546,
on the reasoning that the 5.25 dB would be recovered by the master-volume sweep.
It will not be. The render chain is voices → filter (drive + saturator) →
`dynamics.process` (compressor + saturator) → delay → reverb → **master gain** →
limiter: every level-dependent stage sits *upstream* of the master volume. A
uniform cut at the source moves all of their operating points and no master
adjustment puts them back — it would have silently re-voiced the compressor on
all 45 presets.

The regression is visible: `filter_toggle_over_live_dynamics_is_click_free`
fails under renormalisation, its disengage transient drifting to 8.3× the steady
baseline against a bound of 8×, because the compressor sits further below
threshold and compresses the filter-open jump less. That is not a click — it is
the operating point moving, which is exactly the thing that must not happen.

ADR 0010 §4 is amended accordingly.

## Acceptance criteria

- [x] Ceiling derived from the velocity ladder, not a literal; a test pins it
      against `frac_to_amp(FULL_SCALE_FRAC + vel_level_offset(127, 7))` and
      sweeps level × key scaling × sensitivity × velocity to confirm it is the
      true supremum.
- [x] Nominal stays at unity, so every downstream operating point is preserved.
- [x] A `vel-sens 7` operator at velocity 127 reaches the ceiling and is not
      clamped short.
- [x] Stage 8's clamp is still the only bound, and the lane loop still needs no
      per-sample clamp.
- [x] `filter_toggle_over_live_dynamics_is_click_free` passes unchanged — the
      evidence that the operating point did not move.
- [x] `vxn-asm-check` clean; `stack_tick_stereo` SIMD count unchanged.

## Notes

Peaks rise by up to 5.25 dB on hard-struck patches while nominal is unchanged, so
0326's master-volume sweep still runs — to re-seat peak headroom, not to undo a
uniform shift.
