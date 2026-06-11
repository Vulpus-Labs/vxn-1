---
id: "0077"
title: "Ramp the combined effective level — EG block staircase + mid-block clamp"
priority: high
created: 2026-06-11
epic: E006
depends: ["0076"]
---

## Summary

Sixteenth ticket of [E006](../../epics/open/E006-review-remediation.md).
Two residual artifacts after 0074/0075/0076, found while chasing the
DAW-bounce crackle:

- `eg_tick` is block-rate, so `op.eg.level` was an amplitude staircase
  (~0.01/block at DX7 release rate 67, ~0.11/block at attack rate 96 —
  a buzz at every note edge). This staircase was also the dominant
  term in the "waveform floor" the 0076 alpha was calibrated against.
- Ramping EG and matrix mod independently lets their per-sample SUM
  cross the `[0, 1]` clamp mid-block, re-introducing per-sample clamp
  corners the 0076 target smoothing can't see.

## Design (user-suggested: combine, then ramp)

The effective level the tick reads is `eg + op_level_mod`. Project
everything into that one quantity at block rate and ramp it as a
single per-lane line:

- Targets are computed against the op's post-tick EG level and
  smoothed as in 0076.
- `op_level_mod` is rebased by the EG's block delta
  (`+= prev_eg - eg`) so the sum is continuous at the block edge; the
  EG's motion rides the same ramp as the matrix mod.
- A ramp converging between two in-range endpoints stays in range —
  the per-sample clamp in `stack_tick_*` is now a no-op guard.
- `RAMP_SNAP_EPS` (1e-9/sample) snaps settled ramps onto their
  targets: a converged f32 ramp never compares exactly equal, and
  without the snap `ramp_live` idles on denormal-scale increments
  forever.

Hot path untouched (`stack_tick_*` byte-identical, 0074 discipline).
Block cost: 48 mul-adds per active stack. Measured after the fix the
static d4 floor dropped 16× (3.65e-4 → 2.34e-5) — the old floor WAS
the staircase.

## Acceptance criteria

- [x] Static d4 floor (default patch, no routes, held note) below
  3e-5 — the EG staircase term is gone.
- [x] Gating LFO level route stays within 1.2× of the static floor on
  the amplitude-normalized d4 detector (`level_clamp_corner.rs`).
- [x] `static_patch_keeps_mod_ramps_inactive` — settled EGs release
  the per-sample advance (flat-sustain fixture; the E.PIANO's tails
  legitimately keep the ramp live for ~10 s).
- [x] Convergence test replicates the rebase + smooth recurrence
  independently.
