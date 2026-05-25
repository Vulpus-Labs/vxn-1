---
id: "0022"
title: Param model + routing rewrite (matrix rip-out)
priority: high
created: 2026-05-25
epic: E006
---

## Summary

Foundational rewrite. Replace VXN1's generic **6×4 modulation matrix** with a
small set of **fixed routes** carrying per-channel source selectors, recast
sync/cross-mod as a **type selector + amount**, hardwire the VCA to Env2, turn
Key→Cutoff into a dedicated filter **key-track on/off**, and add **RingLevel**.
This owns the [`vxn-engine::params`] table rewrite and the
[`vxn-engine::lib::build_ctx`] routing rewrite; 0021 (ring) and 0023 (UI) build
on it.

No CLAP id-stability constraint pre-release ([[vxn1-id-stability-dropped]]) —
renumber the `PatchParam` table freely.

## Remove

- The 24 matrix depth params `Env1Pitch … KeyPwm`.
- `modmatrix::{ModSource, ModDest, ModMatrix}` and `PatchParam::{MATRIX_BASE,
  matrix_row_base, matrix_index, is_matrix_param}` (and their tests).
- `OscSync` (bool), `ModWheelDest` + `ModWheelDepth` (replaced below).
- Brown from `NoiseColor` (with 0021); `NOISE_LABELS = ["White","Pink"]`.

## Add (fixed routes)

Per **mod channel** (Pitch, PWM, Cutoff): one LFO selector + depth, one Env
selector + depth.

- `*LfoSrc` enum {Off, LFO1, LFO2}, `*LfoDepth`
- `*EnvSrc` enum {Off, Env1, Env2}, `*EnvDepth`
  for `Pitch`, `Pwm`, `Cutoff` (6 selectors + 6 depths).
- `PitchWheelDepth` (pitch only), `VelCutoffDepth` (cutoff only).
- **Common Pitch channel is vibrato-scaled** — affects *both* oscillators; its
  LFO/env/pitch-wheel depths use a narrow vibrato range (e.g. ±12 st) so the
  knob feel suits vibrato, not sweeps.
- **Wide Osc 2 pitch (sync-sweep) destination** — a *separate* route that pitches
  only osc2 across an **octave range** (multiple octaves; reuse the old
  `ModWheelDepth` ±48 st span). Fed by:
  - `Osc2PitchEnvSrc` enum {Off, Env1, Env2} + `Osc2PitchEnvDepth` (wide).
  - mod-wheel (below).
  Purpose: sweep osc2 against osc1 for sync/cross-mod timbral sweeps, distinct
  from the vibrato pitch channel.
- **Mod-wheel panel** (independent): `ModWheelPwm`, `ModWheelCutoff`,
  `ModWheelReso`, and `ModWheelOsc2Pitch` (octave range — sync sweeps) depths.
- **Filter key-track**: `FilterKeyTrack` (bool). When on, cutoff shifts exactly
  **1 octave per pitch octave above C0** (12 st cutoff per 12 st key).
- **Oscillator interaction**: `CrossModType` enum {Off, Sync, FM} +
  `CrossModAmount` (the old `cross_mod` range/curve). Off = independent fast
  path; Sync drives 0020's band-limited sync; FM drives the exp2 cross-mod.
- **RingLevel** (0..1) for 0021.

Reuse the existing depth ranges/units (pitch in st, cutoff in st, PWM fraction).
Keep the gentle default vibrato by seeding `PitchLfoSrc = LFO1`, `PitchLfoDepth ≈
0.05`, so the default patch sounds as it does today.

## Engine (`build_ctx` + voice mod-source resolution)

- Replace the `ModSource::ALL × ModDest::ALL` loop with explicit per-channel
  resolution: for each channel pick the selected LFO value (LFO1 per-voice /
  LFO2 global / none) and env value (Env1/Env2/none), scale by its depth, sum,
  add the channel's extra (pitch-wheel for pitch, velocity for cutoff, mod-wheel
  contributions from the mod-wheel panel).
- **Pitch is two destinations:** the common pitch sum (both oscs, vibrato scale)
  applies to osc1 and osc2 alike; the wide **osc2-pitch** sum
  (`Osc2PitchEnvDepth` + `ModWheelOsc2Pitch`, octave range) adds to **osc2 only**.
  Both fold into osc2's increment (semitone-domain, same exp2 path as
  coarse/fine/octave), so a sync/cross-mod patch can sweep osc2 over octaves
  while vibrato stays gentle on both.
- VCA amp = Env2 directly (drop the Amp dest entirely).
- Cutoff key-track applies as a hardwired oct/oct term gated by
  `FilterKeyTrack`, independent of the cutoff mod sum.
- `sync` flag now reads `CrossModType == Sync`; `xmod` reads `CrossModType == FM
  ? CrossModAmount : 0.0` (so Off zeroes both, preserving the fast path).

## Acceptance criteria

- [ ] Matrix params + `ModSource`/`ModDest`/`ModMatrix` removed; nothing
      references `matrix_index`/`MATRIX_BASE`.
- [ ] Fixed routes resolve correctly: each channel's selected LFO/env × depth
      sums into its destination; Off selectors contribute nothing.
- [ ] `CrossModType` Off/Sync/FM maps to (independent path / band-limited sync /
      exp2 cross-mod at amount); Off is bit-identical to the fast path.
- [ ] Mod-wheel panel routes (PWM/cutoff/reso/osc2-pitch) work independently of
      the per-channel LFO/env selectors.
- [ ] Common pitch channel is vibrato-scaled and affects both oscs; the wide
      osc2-pitch route (env + mod-wheel, octave range) moves osc2 only — a sync
      patch can sweep osc2 over octaves while vibrato stays gentle.
- [ ] `FilterKeyTrack` on = exactly 1 octave cutoff per octave of key above C0;
      off = no key influence on cutoff.
- [ ] VCA hardwired to Env2; no Amp routing remains.
- [ ] `RingLevel` param present (DSP wired in 0021); brown noise gone.
- [ ] Default patch sounds equivalent to today (default vibrato preserved).
- [ ] Param-table tests updated: contiguous/invertible CLAP ids, defaults in
      range, table length matches `COUNT`.

## Notes

- Big table edit — land before 0023 (UI) and coordinate the `RingLevel` add with
  0021 so the table is rewritten once.
- Update ADR 0003 §5 and the params module docs (the source-major/dest-minor
  matrix description is removed).
- Validation: `cargo test -p vxn-engine`.
