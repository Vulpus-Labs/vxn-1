---
id: "0076"
title: "Round the level-mod clamp corner — residual crackle on gating LFO routes"
priority: high
created: 2026-06-10
epic: E006
depends: ["0074", "0075"]
---

## SUPERSEDED (2026-06-11)

The one-pole target smoother this ticket added was **removed** when level
mod became multiplicative (0078): the bottom gating corner it rounded no
longer exists (a full-depth sine gates through its trough, slope zero), and
an ablation at the shipping 32-sample control rate measured the smoother at
**zero effect** (zipper edge ratio and corner d4 identical with α=1.0).
`LEVEL_TARGET_SMOOTH_ALPHA`/`_EPS` and the `level_mod_smooth` buffer are
gone. The corner regression test (`level_clamp_corner.rs`) and the
multiplicative semantics survive in 0078. Original write-up below for trail.

## Summary

Fifteenth ticket of [E006](../../epics/open/E006-review-remediation.md).
After 0074 (per-sample ramp) and 0075 (CONTROL_BLOCK slicing), pan
routes listen clean but level routes still crackle — specifically at
LFO extremes and when the sound gates to silence (user listening
session, sine/triangle LFO on a carrier level).

Cause: effective op level is `clamp(eg.level + op_level_mod, 0, 1)`
per sample ([stack.rs](../../crates/vxn2-dsp/src/stack.rs) tick fns).
A full-depth bipolar LFO drives the sum past the bounds mid-slope, so
the amplitude envelope hits the clamp at full LFO slope: a first-order
(slope) discontinuity on one sample, twice per LFO cycle. Measured
with a 4th-difference detector (suppresses smooth carriers by f⁴):
isolated broadband events at 6.5e-4, ~3000× the smooth-AM floor,
spaced at half the LFO period. Pan can't do this — equal-power gains
never hard-gate — which matches the listening report exactly.

## Design

Move the saturation to block rate and round it, engine-side, leaving
`stack_tick_*` untouched (0074's lesson: the tick's codegen is
fragile):

- When projecting level dests, clamp each lane's target against the
  op's block EG level: `t' = clamp(eg + t, 0, 1) - eg`. The per-sample
  clamp in the tick becomes a no-op guard (a linear ramp between two
  in-range endpoints stays in range).
- One-pole the clamped target (`LEVEL_TARGET_SMOOTH_ALPHA = 0.25`,
  snap within `1e-5`) so the corner's slope change spreads
  geometrically across blocks instead of landing on one block edge.
- Fresh allocations snap the smoother (shared `mod_seq` generation,
  same as the 0063/0074 state).
- Static patches are bit-identical: zero target + in-range EG clamp to
  themselves and the smoother passes them through.

Alpha choice, measured on a gating 5 Hz full-depth Op1Level route
against the default patch's own waveform sharpness (static-render d4
floor 3.65e-4): raw corner 6.5e-4; α=0.5 → 4.6e-4 (still above
floor); α=0.25 → 3.9e-4 (at floor); α=0.125 → no further gain.
α=0.25 keeps smoothed-motion lag at ~3 blocks (≈2 ms at 48 kHz) and
also slews steppy LFO shapes (Pulse/S&H level jumps spread over ~2 ms
instead of one 0.67 ms ramp).

## Acceptance criteria

- [x] Gating LFO→Op1Level render's max |d4| within 1.2× of the same
  patch's static floor (regression test
  `level_clamp_corner.rs`; pre-fix ratio 1.8×).
- [x] Ramp convergence test updated: per-block convergence onto the
  clamped + smoothed target (recurrence replicated independently in
  the test).
- [x] Static patch keeps `ramp_live` off (existing test still green —
  smoother snap prevents asymptotic creep).
- [x] Manual listen: full-depth sine LFO ~5 Hz on a carrier level,
  patch where the sum gates to silence — no tick at the gate
  boundaries or at LFO extremes.

## Notes

Found in the 0074/0075 listening follow-up 2026-06-10. The d² edge
detector used for zipper can't see this artifact — carrier curvature
at the tremolo maximum masks first-order corners; the d4 detector is
what separated them.

Out of scope, noted while investigating: block-rate `eg_tick` makes
fast DX7-style attacks a ~9-step staircase (~0.11 amplitude per
32-sample block at rate 94) — audible as attack grit on percussive
patches, independent of the mod matrix. Worth its own ticket if it
shows up in listening.
