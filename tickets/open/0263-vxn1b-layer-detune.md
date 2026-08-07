---
id: "0263"
product: vxn-1b
title: "Per-layer detune slider (±50 ct, ±20 ct at half travel)"
priority: medium
created: 2026-08-07
epic: E039
depends: ["0248"]
---

## Summary

The mixer strip now sets a layer's level, mute and placement (0248), but not its
tuning. Detuning one layer against the other is the whole reason a two-layer
synth sounds bigger than one — a few cents apart is the difference between two
layers and one thick one — and today it can only be reached by moving *both*
oscillators' fine tune on one layer, which fights the per-osc detune that Osc 2
Fine is for.

Adds `layer_detune`, a per-layer patch param in cents, applied to the layer's
whole pitch base so both oscillators (and the sub) move together.

## Design

**Param.** `layer_detune`, `[-50, +50]` ct, default `0`, alongside
`layer_level` / `layer_mute` / `layer_pan` in the patch block. Per-layer by
construction, so a preset carries its own detune and the two-layer expansion
gives one instance per synth.

**Application.** `base_semis` in
[synth.rs:284](../../vxn-1b/crates/vxn1b-engine/src/synth.rs#L284) is the layer's
pitch base — master tune plus bend — and is added into both `base1` and `base2`
per voice ([bank.rs:504-505](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L504-L505)).
`layer_detune * 0.01` joins it there: one addition, block-rate, and the whole
layer moves as a unit. Nothing in the voice needs to know.

This is deliberately *not* the same axis as the allocator's per-lane Unison/Twin
detune (which beats a voice against itself) or Osc 2 Fine (which beats the two
oscillators against each other). Those three stack.

**Taper — the interesting part.** A linear ±50 ct slider spends most of its
travel in territory nobody wants: past ~25 ct the two layers read as out of
tune rather than wide, so the musical range is the inner third and it has to be
dialled with a few pixels. The requirement is that **half travel each way reads
±20 ct**, which puts the usable range across most of the slider and keeps the
extremes reachable.

That needs a new taper. `Taper::Exp { mid }` is pinned to `(0, min)`,
`(0.5, mid)`, `(1, max)` — a one-sided curve; a bipolar param needs the same
shape mirrored about centre. Add `Taper::BipolarExp { mid }` to
[vxn-core-app/src/params.rs](../../crates/vxn-core-app/src/params.rs), where
`mid` is the magnitude at half travel **on each side**:

- `to_fader(v)`: `t = curve⁻¹(|v| / max)`, position `0.5 ± t/2`.
- `from_fader(n)`: `u = |2n − 1|`, magnitude `a·(exp(k·u) − 1)` with
  `r = max/mid − 1`, `a = mid/(r − 1)`, `k = 2·ln r`, signed by `n − 0.5`.

Same algebra as the existing zero-floor `Exp` branch, so the shape is the one
already in the codebase, only mirrored. For `max = 50`, `mid = 20`: `r = 1.5`,
`a = 40`, `k = 0.811` — half travel lands on exactly 20 ct and full travel on
50 ct. **The formula requires `mid < max/2`** (`r > 1`); at `mid ≥ max/2` it
divides by zero or flips sign, so the constructor path needs a guard that falls
back to linear rather than emitting NaN into a fader.

Taper is a **view-side** mapping only — `to_normalized`/`from_normalized` stay
linear, so CLAP automation and the preset/state formats are unaffected (the
contract [taper_parity.rs](../../vxn-1b/crates/vxn1b-engine/tests/taper_parity.rs)
already asserts). `taper_to_json` gains the variant in both
[vxn-core-ui-web](../../crates/vxn-core-ui-web/src/lib.rs#L788) and
[vxn1b-ui-web](../../vxn-1b/crates/vxn1b-ui-web/src/lib.rs#L425); the JS reads
norm and never re-derives the curve, so no JS mapping change.

**UI.** `data-control="bipolar"` (the centre-origin horizontal slider built for
matrix depths, 0219) in each mixer strip, under the pan dial. Value text is the
descriptor's own `ct` formatting — the same readout `osc1_fine` and
`unison_detune` already use, sign included; no per-param override.

## Acceptance criteria

- [ ] `layer_detune` exists as a per-layer patch param (`[-50, 50]` ct, default
      0) on both layers; the patch-count and state-version assertions move with
      it.
- [ ] Engine test: layer 2 at `+50 ct` renders measurably sharp against layer 1
      at 0 — e.g. a beat frequency between the two layers where there was none.
- [ ] Engine test: detune moves **both** oscillators and the sub of its layer,
      not just osc 1 — set the two oscs to different waves and confirm both
      shift.
- [ ] Engine test: layer 1's detune does not touch layer 2 (and vice versa).
- [ ] `Taper::BipolarExp { mid }` round-trips: `from_fader(to_fader(v)) ≈ v`
      across `[-50, 50]`, including the exact endpoints and 0.
- [ ] Taper calibration: `from_fader(0.5) == 0`, `from_fader(0.75) ≈ +20 ct`,
      `from_fader(0.25) ≈ −20 ct`, `from_fader(1.0) == +50 ct`,
      `from_fader(0.0) == −50 ct`.
- [ ] Degenerate guard: a `BipolarExp` descriptor with `mid ≥ max/2` falls back
      to linear instead of producing NaN/Inf. Test the boundary directly.
- [ ] `to_normalized`/`from_normalized` stay linear for the new taper — CLAP
      automation and preset round-trips are unwarped (extend
      `taper_parity.rs`).
- [ ] Detune slider present in both mixer strips, bound to the right layer,
      reads cents with its sign; the L2 strip stays gated on layer 2 being on.
- [ ] `layer_detune` round-trips through preset save/load and plugin state.

## Notes

- Stacks with, and is distinct from, `unison_detune` (per-lane, within a voice)
  and `osc2_fine` (per-oscillator, within a layer). Three separate beating
  axes — worth a line in the panel docs so they are not confused.
- The new taper variant lands in the **shared** `vxn-core-app` crate, so
  vxn-2/vxn-3 will see it in their exhaustive `Taper` matches. That is the point
  of putting it there (it is a general bipolar curve, not a VXN1b quirk), but it
  makes this a cross-product change — build all three.
- Classic use to check by ear ([[verify-audio-in-reaper]]): L1 at −7 ct against
  L2 at +7 ct on the same patch, which should read as one wide sound rather than
  two.
