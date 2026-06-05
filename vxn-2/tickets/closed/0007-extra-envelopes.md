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

- [ ] Pitch EG implemented as a 4-segment envelope with the same shape /
      rate semantics as per-op EGs (0001), but levels are signed in
      [−1, +1] (mapped from the −99..+99 plain range).
- [ ] Pitch EG output is fed into the voice's pitch sum with `peg_depth`
      as a global scaler. Default depth 1.0 = the EG can move pitch by
      ±1 semitone × `peg_depth`. (Matrix routing can amplify further.)
- [ ] Mod Env is an ADSR: attack ramps L4→1 over `mod_env_a`, decay 1→S
      over `mod_env_d`, sustain holds at S until gate-off, release S→0
      over `mod_env_r`.
- [ ] Mod Env shape: `Lin` = linear segments. `Exp` = exponential (analog-
      style) curves on A/D/R.
- [ ] Both envelopes added to the per-voice modulation block consumed by
      the matrix (0008).
- [ ] Per-voice (and per stacked-instance) state: each instance has its own
      Pitch EG + Mod Env progression. (Memory cost: 2 envelope states ×
      128 max instances = small.)
- [ ] Test: render a held note with Pitch EG L1=+50, L2=0, R1/R2 fast — the
      output shows a pitch sweep up and back to centre.

## Notes

The Pitch EG and Mod Env share envelope-evaluation code with the per-op EG
(0001). Factor into a `vxn2-dsp::envelope` module: a `EnvSegment` enum and an
`Envelope::tick(state, params) -> f32` that handles both 4-stage-signed
(Pitch EG) and ADSR (Mod Env) shapes via different `EnvParams` variants. Don't
copy-paste.

Default factory patches should rely on the Pitch EG for the snappy initial
pitch envelope characteristic of brassy DX sounds — it's a "free" feature
that's invisible if defaults don't exercise it.
