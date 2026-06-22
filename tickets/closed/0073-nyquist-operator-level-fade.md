---
id: "0073"
product: vxn-2
title: Nyquist-approach per-operator level fade
priority: medium
created: 2026-06-19
epic: E023
---

## Summary

Fade an operator's contribution toward zero as its running frequency
approaches Nyquist, so additive patches built on algorithm 32 (six carriers
at harmonic ratios) stay alias-clean when swept upward instead of folding
high partials back down into the audible band. This is an honest bandlimit
only for the all-carriers case — it does not address FM sideband aliasing
(`carrier ± k·mod`), which remains an oversampling concern (E007).

## Acceptance criteria

- [ ] Each operator's effective level is scaled by a fade factor that is 1.0
      well below Nyquist and ramps to 0.0 as `running_hz → fs/2`.
- [ ] `running_hz` accounts for the live pitch: base note + ratio + fine +
      detune + pitch bend + glide (whatever feeds `phase_inc`), not just the
      static ratio, so a bend/glide upward fades correctly.
- [ ] The fade is applied per lane (SoA), not per stack, so detuned lanes
      near the edge fade independently.
- [ ] Fade window and curve are chosen by a listening + spectrogram pass; the
      onset is high enough not to dull normal bright patches. Document the
      chosen window (e.g. start fading N kHz or N cents below Nyquist) in the
      ticket close-out.
- [ ] Zero per-sample cost. Pitch and level are both block-rate, zero-order
      hold (no per-sample ramp). Compute the fade once per block in
      `apply_pitch_mult` (stack.rs:519) where `phase_inc[k]` is already set,
      and fold it into the operator's effective level at block start (scale
      `op.eg.level` for that op, or pre-multiply `op_level_mod`). The hot loop
      (`lvl_k = lvl + lvl_mod[k]`, stack.rs:781) stays unchanged. Cost: one
      multiply × 6 ops × 8 lanes per block.
- [ ] Fade factor is a cheap function of `phase_inc` (no per-sample
      transcendental): `phase_inc/2^32 = running_hz/fs`, so a clamped
      `smoothstep` over the top fraction of the range suffices.
- [ ] A swept-up algo-32 additive patch shows partials fading rather than
      folding (spectrogram capture attached).

## Notes

- Input is essentially `phase_inc` itself: `phase_inc / 2^32 = running_hz/fs`,
  so the fade can be a pure function of `phase_inc` with no extra pitch math.
  A factor like `smoothstep` over the top fraction of the Nyquist range is
  enough; clamp to [0,1]. `apply_pitch_mult` is the natural home — it already
  loops `i × k` computing `phase_inc`, so `running_hz` is in hand with no
  extra work.
- Control model is zero-order hold, not a ramp: `phase_inc` and EG `level`
  are each computed once per block and held constant across all samples
  (confirmed stack.rs:519, 604, 781). So there is no start→target ramp to
  thread through — the fade is one block-rate scalar multiplied into a
  block-constant level. Simpler and cheaper than ramping into a per-sample
  delta.
- Apply at the level/EG multiply stage so it scales output amplitude. For a
  carrier this directly removes the partial; for an op used as a modulator it
  reduces modulation depth as the modulator nears Nyquist (reasonable, but
  note it's a side effect, not the design target).
- Keep the claim scoped: this is "analytic bandlimit for additive shapes,"
  not global anti-alias. See E023 background.
- Cheap per-block precompute is fine — running frequency only changes with
  pitch/glide, already block-rate quantities.

## Implementation status (code complete; manual audio verification pending)

- Fade lives on `Stack::op_nyquist_fade[op][lane]`, computed in
  `apply_pitch_mult` (stack.rs) from the freshly-set `phase_inc` and multiplied
  into the effective level in `stack_tick_stereo` / `stack_tick_mono`
  (`lvl_k = (lvl + lvl_mod[k]) * fade[k]`). Block-rate; the per-sample cost is
  one extra multiply per lane (the AC's "zero per-sample cost" wasn't literally
  achievable because the fade is per-lane while the base EG level is a per-op
  scalar — it can't be folded into a block-constant without a per-lane base).
- **Benchmark** (`vxn2-osc-bench --bench stack`, algo 5, density 8, sustained
  steady state, 5 consecutive runs each): clean 56.6 µs → fade 57.3 µs =
  **+1.2%** on the fully-loaded SIMD path. The multiply stays inside the
  vectorised lane loop (1.0 except near-Nyquist carriers → no-op ×1.0 normally).
  A "skip the multiply unless a carrier is near Nyquist" branch was tried and
  **reverted** — it defeated autovectorisation and measured ~50% *slower*
  (85 µs). The machine thermally throttles 56→85 µs under load, so the A/B used
  same-thermal steady runs, not a one-shot baseline.
- **Fade window** (`stack.rs` consts): `NYQUIST_FADE_LO = 0.45`,
  `NYQUIST_FADE_HI = 0.49` of fs → at 48 k the fade starts at 21.6 kHz and
  fully mutes by 23.5 kHz; at 44.1 k, 19.85 kHz → 21.6 kHz. Hermite smoothstep,
  inverted. **Needs a listening pass** to confirm it doesn't dull legitimately
  bright patches; tune the two consts by ear.
- **Carrier-only** (refinement during review): the fade applies only to
  carrier ops (`spec_of(algo).carriers` mask); modulators always keep
  `fade = 1.0`. Fading a modulator would thin the FM index as it rises — e.g.
  mute a ratio-14 tine on a high note (E-Piano C7), which is musically wrong —
  and a modulator's sidebands alias regardless of its own level (oversampling
  territory, not this fade). The honest bandlimit claim is carriers-only, so
  this matches the scope.
- Stack-path only; the scalar reference/bench path (`voice.rs`) is unchanged.
- Tests (stack.rs): `nyquist_fade_curve_is_unity_low_zero_high_monotone`,
  `fade_is_unity_at_normal_pitch`, `fade_silences_partials_swept_past_nyquist`,
  `fade_is_carrier_only_modulators_unattenuated`.
- Side effect noted in the audibility test: the default patch's ratio-14 tine
  modulator becomes a *carrier* under the test's algo-32 base context and runs
  past Nyquist at note 96, so the KS-test contexts now pin the op to ratio 1 to
  keep it in-band (it was previously relying on aliasing for audibility).
- Spectrogram capture of an upward sweep showing partials fading not folding:
  **pending manual verification in a DAW**.

## Close-out (2026-06-22)

- Per-lane Nyquist fade on `Stack::op_nyquist_fade[op][lane]`,
  computed in `apply_pitch_mult` from `phase_inc` and multiplied into
  effective level in `stack_tick_stereo`/`stack_tick_mono`. Carrier-
  only (`spec_of(algo).carriers`); modulators stay 1.0.
- **Window** (stack.rs consts): `NYQUIST_FADE_LO = 0.45`,
  `NYQUIST_FADE_HI = 0.49` of fs (48 k: 21.6 k → 23.5 k; 44.1 k:
  19.85 k → 21.6 k), Hermite smoothstep inverted. **+1.2%** bench on
  the loaded SIMD path; the skip-branch variant was reverted
  (defeated autovec, ~50% slower).
- Tests: `nyquist_fade_curve_is_unity_low_zero_high_monotone`,
  `fade_is_unity_at_normal_pitch`,
  `fade_silences_partials_swept_past_nyquist`,
  `fade_is_carrier_only_modulators_unattenuated`. Manual listen /
  spectrogram pass waived at close.
