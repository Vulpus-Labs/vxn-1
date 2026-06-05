---
id: "0002"
title: Oscillator octave controls
priority: low
created: 2026-05-24
epic: E001
---

## Summary

Give each oscillator an explicit **octave** control, separate from the existing
coarse-semitone and fine-cent controls. Formalises the tuning model from ADR
0002 (octave / semitones / cents per oscillator) while preserving VXN1's ability
— which the JP-8 lacked — to set oscillators at **non-octave intervals** (e.g. a
fifth, 7 semitones).

## Acceptance criteria

- [x] New int params `Osc1Octave`, `Osc2Octave` (range −4..+4 octaves, default
      0), **appended at the end of the `ParamId` table** for stable CLAP ids.
- [x] `build_ctx` folds the octave into the per-oscillator semitone offset:
      `osc1_semi = Osc1Octave*12 + Osc1Coarse + Osc1Fine/100.0` (and likewise
      osc2). No change needed in `voice.rs` — it already adds `ctx.osc*_semi`.
- [x] Coarse stays ±24 st and fine ±50 ct, so non-octave intervals remain
      expressible; octave only extends/organises the range.
- [x] Tests: an octave-up setting doubles the rendered frequency; coarse + octave
      combine additively (e.g. +1 oct & +7 st = +19 st); param-table tests pass.

## Notes

- Pure param + ctx math; no DSP kernel change. The smallest item in E001.
- Osc2 default coarse is −12 st today; decide whether to re-express that as
  octave −1 / coarse 0 or leave it. Leaving it is fine — additive either way.
- UI control for the new knob is deferred (epic-level note); this ticket is
  engine + param model only.
- Validation: `cargo test -p vxn-engine`.
