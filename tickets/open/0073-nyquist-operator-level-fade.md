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
