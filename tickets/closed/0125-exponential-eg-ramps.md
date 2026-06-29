---
id: "0125"
product: vxn-2
title: "Exponential EG ramps for the Exp curve path (log-domain march)"
priority: medium
created: 2026-06-23
epic: E026
depends: ["0124"]
---

## Summary

Third ticket of [E026](../../epics/open/E026-dx7-log-level-curve.md). 0123/0124
make the level *targets* logarithmic but the EG still **marches linearly in
amplitude** toward them, so a decay to a now-much-lower target stretches in time
and the attack/decay *shape* isn't DX7's. Implement log-domain marching
(linear-in-dB ramp → exponential amplitude) for the `Exp` curve, the
characteristic DX7 tapered envelope.

## Design

- `eg.rs`: when `curve == Exp`, march the EG state in log2 units (linear ramp
  toward a log2 target at a dB/sec rate); output amplitude = `2^(level)` once per
  control tick. Keep `eg.level` (amplitude) as the value the lane loop reads.
- Recalibrate rates for the log domain: `rate_to_*` so segment times feel like
  DX7 (R=0 ≈ 20 s, R=99 ≈ 4 ms full sweep), verified by listening.
- `Lin` path unchanged (linear-amplitude ramp toward square targets).

## Acceptance criteria

- [ ] `Exp` EG marches in log2 domain; amplitude is `exp2` of the marched value.
- [ ] Rate curve recalibrated; attack/decay times sane across R=0..99 (listening
      check on an e-piano pluck, a pad swell, a bass).
- [ ] `Lin` path bit-identical to pre-0125 behavior.
- [ ] Stays **control-rate scalar** — the `exp2` is per-op per-block, never in
      the per-sample lane loop. Confirm via idle + full-poly benches (no
      regression) and an asm/CPU spot-check.
- [ ] The `op{N}-level` matrix destination (writes `eg.level` as amplitude —
      `stack.rs ~696`) still behaves correctly with the log-domain state.

## Notes

`EgState` likely needs a log-domain state field plus a stored `max_amp` (the
linear ceiling = OL × ks × vel applied after `exp2`). Watch the matrix op-level
write path and `force_sustain`. This is the only part of the epic that touches
the per-sample-adjacent code, hence the explicit bench gate.

## Implementation note (2026-06-24) — code landed, **pending ear verification**

Implemented in [eg.rs](../../vxn-2/crates/vxn2-dsp/src/eg.rs); **ticket stays
open until verified by ear in Reaper** (epic AC). What landed:

- `EgState` gained `curve`, `max_amp`, `log_level`, `log_targets[4]`,
  `log_rates[4]`, and a `kill` flag. `Exp` marches the **downward** segments
  (Decay1/Decay2/Release) linearly in log2 → exponential amplitude taper
  (`level = max_amp × 2^log_level`), the DX7 shape. **Attack stays a
  linear-amplitude rise** (DX7 attack is fast/punchy; a log creep from the
  −90 dB floor would be a dead-then-pop attack) — at the attack→Decay1
  transition `log_level` is seeded from the reached amplitude. `Lin` is
  unchanged (marches `level` in amplitude every segment).
- `kill_release` (declick) is a linear-amplitude ramp to 0 on **both** curves
  via the `kill` flag — smooth+fast is all a declick needs; the Release stage
  ignores the log marcher while killed.
- Rate recalibration: `rate_to_log2_per_sec` is built so a **full** segment
  sweep takes the *same* wall-clock as the old linear march
  (`20 / 2^(R/8)` s; R=0 ≈ 20 s, R=99 ≈ 4 ms). Only the *shape within* a
  segment changes (constant dB/sec vs constant amp/sec), so a decay-to-silence
  patch is much lower mid-tail.
- `op{N}-level` matrix dest unaffected: the hot loop reads `eg.level` (amplitude)
  + `op_level_mod` additively (`stack.rs` ~963/983) — `eg.level` is amplitude on
  both curves, so that path is unchanged.
- Perf: the log marcher + `exp2` run in `eg_tick` (control rate, scalar, once
  per op per block) — the per-sample NEON lane loop is byte-unchanged. No hot-path
  edit ([[vxn1-soa-match-defeats-simd]]); the explicit bench run is still owed.

Tests (dsp): `exp_decay_is_linear_in_db`, `lin_decay_is_linear_in_amp`,
`exp_rate_zero_is_far_slower_than_max`, `kill_release_declicks_linearly_on_exp`;
the existing attack/decay/sustain/release tests pass under the new Exp path.
Engine `default_patch_renders_with_expected_envelope` retargeted (the percussive
E.PIANO now decays exponentially → a lower, still-ringing mid-tail at t≈1 s).
Also fixed a **pre-existing** (5684c2d, the curve prototype) `param_sweep`
fixture, `silence_when_master_volume_min`, which leaked the note-on transient
through the still-ramping master smoother — added an 8-block pre-roll so it tests
settled −60 dB silence.

**CPU bench gate (2026-06-24, M-series, criterion median):** no regression —
`vxn2-osc-bench` pre-0125 (`d42b6a0^`) vs post:
`stack_d1` 63.0→60.6 µs, `stack_d4` 62.4→60.7, `stack_d8` 62.4→60.5 (all ~−3%,
within noise / marginally faster); `voice_release` 38.6→38.9 µs (+0.6%, noise).
`voice_steady` pre-run was a noisy outlier (wide CI 55–58 µs); post 38.7 µs
matches `voice_release`, i.e. effectively equal. Confirms the log marcher +
`exp2` are control-rate scalar — the per-sample lane loop is byte-unchanged.

**To verify (Reaper):** attack feel (punchy, not soft), decay/release taper
(natural exponential, not linear), segment times across R=0..99 on an e-piano
pluck, a pad swell, and a bass. If times feel off, tune `rate_to_log2_per_sec`'s
`/20.0`; if attack feels wrong, that's the linear-attack choice to revisit.
dsp 188 / engine 205 lib green; CPU benches still to run.

## Close-out (2026-06-27)

- **Ear-verified in Reaper** (epic AC, the sole remaining gate): attack stays
  punchy (linear-amplitude rise preserved), decay/release taper reads as natural
  exponential, segment times sane across R=0..99 on an e-piano pluck, a pad
  swell, and a bass. No retune of `rate_to_log2_per_sec` needed; linear-attack
  choice confirmed correct.
- Code landed at [eg.rs](../../vxn-2/crates/vxn2-dsp/src/eg.rs): `Exp` marches
  Decay1/Decay2/Release linearly in log2 → exponential amplitude
  (`level = max_amp × 2^log_level`); attack linear; `Lin` path unchanged;
  `kill_release` declick linear on both curves.
- Tests green: `exp_decay_is_linear_in_db`, `lin_decay_is_linear_in_amp`,
  `exp_rate_zero_is_far_slower_than_max`, `kill_release_declicks_linearly_on_exp`
  (dsp), plus engine `default_patch_renders_with_expected_envelope`.
- CPU bench gate passed (M-series, criterion median): no regression — `stack_d1/d4/d8`
  ~−3% (noise/marginally faster), `voice_release` +0.6% (noise). Control-rate
  scalar `exp2`; per-sample NEON lane loop byte-unchanged.
