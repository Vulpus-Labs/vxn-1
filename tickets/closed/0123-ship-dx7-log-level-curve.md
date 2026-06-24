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

## Close-out (2026-06-24)

- The log curve `amp = 2^((L-99)/8)` is the shipped default for both the EG
  L-values and the operator output level — landed in commit `5684c2d` and routed
  through [eg.rs](../../vxn-2/crates/vxn2-dsp/src/eg.rs) `level_to_amp` →
  [op.rs:157](../../vxn-2/crates/vxn2-dsp/src/op.rs#L157) +
  [stack.rs:861](../../vxn-2/crates/vxn2-dsp/src/stack.rs#L861) cook.
- ADR added: [adrs/0007-dx7-log-level-curve.md](../../vxn-2/adrs/0007-dx7-log-level-curve.md)
  — documents the linear/square-vs-log divergence, the 6 dB/8-step curve
  (L=0 → silence), calibration (L=50 ≈ −37 dB), and the recalibration policy
  (DX7-faithful values are now correct; old hand-tunings revert — precedent
  Mark II E-Piano tine 17 → 58).
- `EG_LOG_LEVELS` prototype const removed — superseded by the per-op `eg-curve`
  param (default `Exp` = this log curve) under **0124**, exactly as that
  ticket's Notes specify. `eg::level_to_amp` now takes an `EgCurve`; the const is
  gone.
- Curve is computed only in `cook` (control rate, scalar); the per-sample NEON
  lane loop reads the precomputed `eg.level` scalar — SIMD/perf neutral
  ([[vxn1-soa-match-defeats-simd]]). dsp 184 / engine 205 lib tests green.
  (Audibility guard handled in 0127; per-op param + tests in 0124.)
- Pre-existing, unrelated: `vxn2-clap` `process_loop_two_batch_render_*` and
  `reset_silences_held_voice` panic `index out of bounds: len 32 index 32` on
  clean `HEAD` too — not from this work.
