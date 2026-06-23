---
id: "0123"
product: vxn-2
title: "Ship the DX7 logarithmic operator/EG level curve as the engine default"
priority: high
created: 2026-06-23
epic: E026
---

## Summary

First ticket of [E026](../../epics/open/E026-dx7-log-level-curve.md). vxn-2 maps
operator output level linearly (`op.rs`/`stack.rs` cook: `(level/99)`) and EG
L-values via a square curve (`eg.rs level_to_amp: (L/99)^2`). DX7 is logarithmic
(~0.75 dB/step), so mid-level modulators were ~30× too hot — the root cause of
the bank-wide brightness/buzz. Make the log curve `amp = 2^((L-99)/8)` the
shipped default for both the EG L-values and the operator output level, and
record the decision in an ADR.

A working-tree prototype already implements this behind the `EG_LOG_LEVELS`
const and is confirmed a major improvement by listening.

## Design

- `eg.rs`: `level_to_amp(L)` = `if L==0 {0} else {2^((L-99)/8)}` (0 dB at 99,
  −6 dB per 8 steps, ≈ −74 dB at L=1). Retire the `EG_LOG_LEVELS` prototype
  const — the curve becomes the default (the per-op Lin escape hatch arrives in
  0124, which is where Lin/square will live).
- `op.rs:149` + `stack.rs:752` cook: `level_norm = eg::level_to_amp(params.level)`
  (operator output level shares the EG level curve, as on DX7).
- The curve is computed only in `cook` (control rate) — the per-sample lane loop
  is untouched, so this is SIMD/perf neutral.

## Acceptance criteria

- [ ] EG L-values and operator output level both use the log curve by default.
- [ ] `EG_LOG_LEVELS` prototype const removed.
- [ ] ADR added documenting: the linear/square-vs-log divergence, the chosen
      curve (6 dB / 8 steps; L=0 → silence), calibration vs DX7 (L=50 ≈ −37 dB),
      and the **recalibration policy** — faithful DX7 levels are now correct, so
      any preset hand-tuned for the old linear engine must be restored to DX7
      values (precedent: Mark II E-Piano tine 17 → 58).
- [ ] vxn2-dsp tests green; vxn2-engine tests green (the audibility guard is
      handled in 0127 — coordinate).
- [ ] Idle + full-poly CPU benches show no regression.

## Notes

Prototype lives in `vxn-2/crates/vxn2-dsp/src/{eg.rs,op.rs,stack.rs}`. The
EG ramp *shape* stays linear-in-amplitude in this ticket (exponential ramps are
0125); only the level→amplitude mapping changes here.
