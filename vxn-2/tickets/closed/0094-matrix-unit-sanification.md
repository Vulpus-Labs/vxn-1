---
id: "0094"
title: "Unit sanification: normalize source outputs + recalibrate dest gains"
priority: high
created: 2026-06-12
epic: E008
depends: []
---

## Summary

Fifth ticket of [E008](../../epics/open/E008-mod-matrix-completeness.md). Make
matrix units coherent. Today source outputs mix ranges and one dest path
double-scales:

- Bipolar sources (`lfo1`, `lfo2`, `voice-spread`) are `[-1, 1]`; unipolar
  (`mod-wheel`, `aftertouch`, `velocity`, `key`, `mod-env`, `voice-idx`,
  `voice-rand`) are `[0, 1]`; `pitch-eg` is **raw semitones**
  ([matrix.rs:537](../../crates/vxn2-engine/src/matrix.rs#L537)).
- `pitch-eg → *-pitch` / `global-pitch` then multiplies those semitones by the
  pitch dest's `24.0` gain ([DEST_GAIN](../../crates/vxn2-engine/src/matrix.rs#L279))
  **and** the cubic depth taper — so a 1-semitone EG at unity depth swings ~24
  semitones. That's a unit bug, not a feature.

Normalize source outputs to documented ranges and recalibrate `DEST_GAIN` so
`depth = 1` means a sensible full-scale in each dest's native unit, with the
source contributing a clean `[-1, 1]` (or `[0, 1]`) shape.

## Design

**Source normalization** ([eval_sources](../../crates/vxn2-engine/src/matrix.rs#L575)):

- Define each source's canonical output range and document it on `LaneSources` /
  the source tables. Target: bipolar sources → `[-1, 1]`, unipolar → `[0, 1]`.
- `pitch-eg`: divide the raw semitone output by the pitch EG's configured depth
  (its max excursion) so the source emits a normalized `[-1, 1]` *shape*, not
  absolute semitones. The pitch dest's gain then sets the actual semitone span.
  (Confirm the pitch-EG depth param is reachable at the source-eval site; if not,
  carry the normalization factor on `StackScalarSources`.)
- This makes `pitch-eg` behave like every other source: a shape into a
  gain-scaled dest. A user wanting "EG drives pitch ±2 oct" sets depth on the
  slot, not via a hidden 24× on a semitone value.

**Dest gain recalibration** ([DEST_GAIN](../../crates/vxn2-engine/src/matrix.rs#L277)):

- Audit every entry. Each dest's `depth = 1` full-scale, in native units:
  - pitch dests (`*-pitch`, `global-pitch`): keep ±24 st (±2 oct) — but now fed
    a normalized source, so `pitch-eg → global-pitch` at depth 1 = ±2 oct
    *shaped by the EG*, not 24× the EG's semitones.
  - `*-level`: multiplicative on EG ([engine.rs:582](../../crates/vxn2-engine/src/engine.rs#L582)),
    keep `1.0` (full-depth = full tremolo gate).
  - `*-pan`: keep `1.0` (full-depth = hard L↔R).
  - `feedback`: keep `7.0` (covers the 0..7 clamp).
  - `cutoff`: keep `8.0` octaves; `resonance`: keep `1.0` additive.
  - `lfo1-rate` / `lfo2-rate` (new in 0092): octave span, e.g. `±4` — set the
    value here so 0092 consumes it.
  - `stack-detune` / `stack-spread` (new in 0093): full-scale = the macro's
    native range; set so `depth = 1` reaches the macro's full sweep.
- Document the unit + full-scale of every dest in `PARAMETERS.md` and the
  `DEST_GAIN` doc comment as a table (dest → unit → depth-1 span).

**Depth taper:** the cubic taper ([cook_depth](../../crates/vxn2-engine/src/matrix.rs#L388))
currently applies only to pitch dests. Re-evaluate whether the rate/detune dests
(also wide-range, log-ish) want the same low-end-widening taper; document the
decision. Non-pitch dests stay linear unless shown to need it.

**Behaviour change:** this *will* alter the sound of any patch routing `pitch-eg`
into pitch (the 24× collapse), and any patch relying on a now-changed gain. That
re-audit is [0097](0097-preset-reaudit-matrix-tests.md). A blob-version bump is
**not** needed (depths are stored normalized and unchanged; only the
runtime interpretation shifts) — but call out the audible delta in the migration
notes so it's not mistaken for a regression.

## Acceptance criteria

- [x] `pitch-eg` now emits a normalized `[-1, 1]` shape (`level_st /
  peg_depth`), not raw semitones — `pitch_eg_source_is_normalized_shape` asserts
  the source stays in `[-1, 1]` even at `peg_depth = 7`. The other sources were
  already normalized (`[-1,1]` bipolar / `[0,1]` unipolar) and are structurally
  unchanged (covered by `eval_sources_broadcasts_scalars_and_keeps_lane_values`).
- [x] `pitch-eg → global-pitch` at depth 1 reaches ±24 st shaped by the EG, not
  `peg_depth × 24`: `matrix_pitch_eg_into_pitch_no_double_scale` pins +24/−24 st
  at `peg_depth = 2` (the old path gave 48).
- [x] `DEST_GAIN` audited; every dest's `depth = 1` full-scale documented as a
  unit table in both `PARAMETERS.md` and the `DEST_GAIN` doc comment. Rate dests
  ±4 oct (0092), stack macros `(1 + v)` scale (0093) — confirmed/listed.
- [x] Cubic-taper applicability decided + documented: taper stays on the 7
  semitone pitch dests only; log-domain rate/cutoff and `(1+v)`-scale stack
  macros stay **linear** (a taper would double-bend already-shaped gains).
- [x] Migration note in `PARAMETERS.md` records the `pitch-eg → *_pitch` audible
  change + re-scale guidance (hand-off to 0097). No blob-version bump — depths
  stored normalized + unchanged; the full workspace round-trip/snapshot tests
  still pass.
- [x] No RT alloc / unwrap / panic; `eval_dests` is unchanged in shape (curve
  dispatch + lane loop intact) — normalization is one division per stack per
  block at the source-build site, outside the per-slot loop.

## Notes

The win is conceptual uniformity: **every source is a normalized shape, every
dest's gain converts that shape to its native unit, depth scales the shape.** No
source carries hidden units that a dest then re-scales. This is also what lets
the validator (0095) and the bipolar fader (0096) show a meaningful, comparable
"amount" readout across dests.
