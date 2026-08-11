---
id: "0248"
product: vxn-1b
title: "Per-layer pan knob in the mixer strip (constant-power law)"
priority: medium
created: 2026-08-07
epic: E039
depends: ["0220"]
---

## Summary

The layer mixer built in **0220** gives each layer a level fader and a mute, but
no pan. Everything downstream of the voice is already stereo — each synth renders
into `bus_l`/`bus_r` ([engine.rs:566](../../vxn-1b/crates/vxn1b-engine/src/engine.rs#L566)),
unison spread already pans lanes across the image
([bank.rs:627-632](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L627-L632)), and
the meter taps are already `(L, R)` pairs
([meters.rs:37](../../vxn-1b/crates/vxn1b-engine/src/meters.rs#L37)) — so placing
the two layers apart in the image costs one multiply per sample in a loop that
already exists. A dual-layer synth without per-layer placement is a mixer strip
missing its most-used control: the classic use is a wide pad panned against a
narrow lead.

Adds `layer_pan` as a per-layer patch param, applied in the same per-sample loop
as the layer mix gain, ahead of the existing post-fader meter tap.

Extends [E039](../../epics/open/E039-vxn1b-dual-layer.md), which specced the
mixer strip as level + mute only.

## Design

**Param.** New `layer_pan` beside `layer_level`/`layer_mute` in the patch table
([params.rs:564](../../vxn-1b/crates/vxn1b-engine/src/params.rs#L564)), bipolar
`[-1, 1]`, default `0.0`, `Taper::Linear` — the same shape as the existing
bipolar matrix-depth descriptors. Because it is a per-layer patch param, the
outer two-layer map picks it up on both layers automatically: 160 → 162 CLAP
params, so the count assertion at
[vxn1b-clap/src/lib.rs:751](../../vxn-1b/crates/vxn1b-clap/src/lib.rs#L751) moves.

**Law.** Constant power, normalised to unity at centre:
`gl = √2·cos(θ)`, `gr = √2·sin(θ)` with `θ = (pos + 1)·π/4`. `gl² + gr²` is
constant across the whole sweep — the point of the law, so a layer keeps its
apparent loudness as it crosses the image — and the `√2` puts centre at exactly
`1.0` per channel rather than the textbook `0.707`. The normalisation is free
(it is still constant power, just a different reference point) and it means a
centred patch renders exactly as it does today.

The cost of any constant-power law is at the extremes: a hard-panned layer puts
`1.414 ×` the centre amplitude into one channel, i.e. 3 dB of extra peak. That is
inherent to holding power constant and is what the master limiter (0251) is for;
it is not a reason to fall back to a linear law.

**Application.** `layer_gain: [Smoothed; 2]` becomes an L/R pair per layer, with
targets `gain · gl` and `gain · gr` — smoothing the *product* rather than the
pan position, so one `Smoothed::tick` per channel per frame covers both a fader
move and a pan sweep, and the existing mute-fade semantics
([engine.rs:509-522](../../vxn-1b/crates/vxn1b-engine/src/engine.rs#L509-L522))
carry over untouched. The two scale loops — layer 1 in place on the output
buffers, layer 2 in `mix_scratch` — take the per-channel gain instead of one
shared `g`.

**Mono fast path.** Not this ticket's problem — the `spread_zero` hint that skips
the R decimator lives in the oversampling work (0251), which is not on main yet.
Removing it is **0262**, which this ticket does not depend on: at OS 1× there is
no decimator to skip, so layer pan is correct on today's main either way. 0262
must land before 0251's fast path can reach main with pan present.

**Meters.** The post-fader tap at
[engine.rs:581](../../vxn-1b/crates/vxn1b-engine/src/engine.rs#L581) sits after
the scale loop, so it becomes post-pan for free — a hard-left layer reads L-only
on its strip, which is what a mixer strip should show. No view work: the widget
is already stereo.

**UI.** A `data-control="dial"` on `layer_pan` in each `.mixer-strip`
([faceplate.html:277-297](../../vxn-1b/crates/vxn1b-ui-web/assets/faceplate.html#L277-L297)),
with `data-fixed-layer="upper"`/`"lower"` like the level fader beside it. Sits
under the fader, above the mute. Value text reads `L50` / `C` / `R50`.

## Acceptance criteria

- [ ] `layer_pan` exists as a per-layer patch param (bipolar, default centre) and
      appears on both layers in the outer CLAP map; the total-param assertion in
      `vxn1b-clap` is updated to the new count and passes.
- [ ] Engine test: with layer 1 panned hard left and layer 2 hard right, a block
      renders layer 1's signal only in L and layer 2's only in R.
- [ ] Engine test: at centre, L and R gains are both exactly the layer level
      (unity normalisation), and `gl² + gr²` is constant within tolerance at pan
      `-1`, `-0.5`, `0`, `0.5`, `1`.
- [ ] Engine test: `spread == 0` with a non-centre `layer_pan` produces `L != R`
      — the case 0262 has to keep working once the mono fast path arrives.
- [ ] A pan move ramps rather than steps: no discontinuity at the control-block
      boundary.
- [ ] Post-fader meter tap is post-pan: a hard-panned layer reads on one channel
      only.
- [ ] Pan dial present in both mixer strips, bound to the right layer, value text
      round-trips (`L50`/`C`/`R50`); the L2 strip stays gated on layer 2 being on.
- [ ] `layer_pan` round-trips through preset save/load and plugin state.

## Notes

- Existing patch compatibility is explicitly not a constraint on this work. The
  unity-centre normalisation happens to preserve it anyway, so no factory audit
  is needed — but a law change that did move levels would have been acceptable.
- Not the same control as `Spread`, which pans unison *lanes within* a layer
  ([bank.rs:829](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L829)). **0260**
  turns that into a matrix source→dest pair and moves the voice pan to the same
  constant-power law; this ticket is the static per-layer control it multiplies
  into.
- Built on main, which has no oversampling yet: the mix loop is base-rate, one
  `Smoothed::tick` per frame. When 0251's OS work lands, the per-channel gains
  must be held across each base frame's OS sub-samples, exactly as the single
  gain is there today.
- Out of scope: per-layer stereo width.
- Meter view work is genuinely nil — see [[vxn-metering-spine]]; the spine was
  built stereo-per-tap in 0240.
- [[vxn1b-two-layer-param-map]]: tests that touch the new id must use
  `clap_id_of`, not `as usize`.

## Close-out (2026-08-11)

- `layer_pan` exists as a per-layer patch param, bipolar `[-1, 1]`, default
  centre, `Taper::Linear`
  ([params.rs:585](../../vxn-1b/crates/vxn1b-engine/src/params.rs#L585),
  `ParamId::LayerPan` at [params.rs:186](../../vxn-1b/crates/vxn1b-engine/src/params.rs#L186)),
  picked up on both layers by the outer two-layer map.
- Constant-power law normalised to unity at centre in `pan_gains`
  ([engine.rs:286](../../vxn-1b/crates/vxn1b-engine/src/engine.rs#L286)); the
  per-layer gain became an L/R pair so one `Smoothed::tick` per channel per frame
  covers both a fader move and a pan sweep, and the mute-fade semantics carry
  over untouched.
- Engine coverage, all green:
  `pan_law_is_constant_power_with_unity_at_centre`
  ([engine.rs:1487](../../vxn-1b/crates/vxn1b-engine/src/engine.rs#L1487)),
  `layers_panned_apart_land_in_opposite_channels`
  ([engine.rs:1518](../../vxn-1b/crates/vxn1b-engine/src/engine.rs#L1518)),
  `centre_pan_leaves_the_channels_identical`
  ([engine.rs:1575](../../vxn-1b/crates/vxn1b-engine/src/engine.rs#L1575)),
  `panning_a_layer_does_not_step_the_output`
  ([engine.rs:1589](../../vxn-1b/crates/vxn1b-engine/src/engine.rs#L1589)),
  `a_muted_layer_is_silent_at_any_pan`
  ([engine.rs:1623](../../vxn-1b/crates/vxn1b-engine/src/engine.rs#L1623)).
- Post-fader meter tap is post-pan for free — the scale loop runs ahead of it.
- Preset + state round-trip via
  `preset::tests::layer_pan_round_trips_and_stays_sparse_at_centre`
  ([preset.rs:508](../../vxn-1b/crates/vxn1b-engine/src/preset.rs#L508)) — a moved
  pan is written, a centred one stays out of the sparse TOML. State `VERSION` 7
  ([state.rs:55](../../vxn-1b/crates/vxn1b-engine/src/state.rs#L55)).
- Faceplate: `layer_pan` dial in both mixer strips
  ([faceplate.html:290](../../vxn-1b/crates/vxn1b-ui-web/assets/faceplate.html#L290),
  [:304](../../vxn-1b/crates/vxn1b-ui-web/assets/faceplate.html#L304)); view
  coverage in `layer-pan.test.js`.
- **Ordering note — the ticket's "Mono fast path" section was inverted by
  events.** It assumed 0251 was not yet on main; it was, so the `spread_zero`
  hint would have silently discarded pan at OS ≥ 2. 0262 therefore landed
  *before* this ticket (c496638 → 9cf0363), not after, and the
  `spread == 0 with non-centre layer_pan ⇒ L != R` criterion is asserted there.
- Shipped in 9cf0363.
