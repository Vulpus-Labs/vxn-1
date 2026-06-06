---
id: "0007"
title: Pitch EG + Mod Env (extra envelope sources)
priority: medium
created: 2026-06-05
epic: E001
---

## Summary

Two patch-wide modulation envelopes beyond per-op EGs:

- **Pitch EG**: 4-rate / 4-level with *signed* levels (positive and negative
  pitch excursions). Default routes additively to global pitch.
- **Mod Env**: 4-stage ADSR with shape selector (Lin / Exp). General-purpose
  matrix source. No default routing.

Both are per-voice (retriggered at note-on, released at note-off).

## Acceptance criteria

- [x] Pitch EG implemented as a 4-segment envelope with the same shape /
      rate semantics as per-op EGs (0001), but levels are signed in
      [−1, +1] (mapped from the −99..+99 plain range).
- [x] Pitch EG output is fed into the voice's pitch sum with `peg_depth`
      as a global scaler. Default depth 1.0 = the EG can move pitch by
      ±1 semitone × `peg_depth`. (Matrix routing can amplify further.)
- [x] Mod Env is an ADSR: attack ramps L4→1 over `mod_env_a`, decay 1→S
      over `mod_env_d`, sustain holds at S until gate-off, release S→0
      over `mod_env_r`.
- [x] Mod Env shape: `Lin` = linear segments. `Exp` = exponential (analog-
      style, one-pole approach, `tau = secs/4.6`) curves on A/D/R.
- [x] Both envelopes added to the per-voice modulation block consumed by
      the matrix (0008). State lives on [`Voice`] + [`Stack`]; matrix reads
      `pitch_eg.level_st` (semitones) and `mod_env.level` (`[0, 1]`).
- [x] Per-voice state: each [`Voice`] / [`Stack`] has its own Pitch EG +
      Mod Env progression. Shared across the 8 stack lanes — same precedent
      as the per-op EG. If matrix routing ever needs per-lane scattering, it
      applies a per-lane scaler at consumption time without reshaping state.
- [x] Test: render a held note with Pitch EG L1=+50, L2=0, R1/R2 fast — the
      output shows a pitch sweep up and back to centre
      (`stack::tests::pitch_eg_lifts_phase_inc_then_settles`).

## Notes

The Pitch EG and Mod Env share envelope-evaluation code with the per-op EG
(0001). Factor into a `vxn2-dsp::envelope` module: a `EnvSegment` enum and an
`Envelope::tick(state, params) -> f32` that handles both 4-stage-signed
(Pitch EG) and ADSR (Mod Env) shapes via different `EnvParams` variants. Don't
copy-paste.

Default factory patches should rely on the Pitch EG for the snappy initial
pitch envelope characteristic of brassy DX sounds — it's a "free" feature
that's invisible if defaults don't exercise it.

## Implementation notes (post-close)

- New module `vxn2-dsp::envelope` with two concrete state machines
  ([`PitchEgState`], [`ModEnvState`]) plus shared `march_linear` /
  `march_exp` helpers. The single-`Envelope` enum sketched in the
  pre-close Notes was rejected: PEG and Mod Env need different `EnvParams`
  shapes (rates vs ms), different output ranges (semitones vs `[0, 1]`),
  and different stage sets (4-stage with `Decay2` vs ADSR). Two types
  with shared inline helpers reads cleaner and didn't trigger any
  copy-paste between them.
- Rate semantics for PEG reuse [`crate::eg::rate_to_amp_per_sec`]
  unchanged — the per-op EG's 0..99 → log-spaced amp/sec mapping.
- `Stack::eg_tick` now ticks per-op EGs + PEG + Mod Env then calls
  `apply_pitch_mult` to fold the PEG semitone offset into `phase_inc`.
  Same wiring on the scalar `Voice`. Per-sample loop sees the cooked
  `phase_inc` without per-sample envelope work.
- Factory-default exercise of the PEG (the "free brassy attack" hook in
  the original Notes) is deferred to ticket 0028 (factory bank).
