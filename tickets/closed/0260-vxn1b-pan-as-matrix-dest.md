---
id: "0260"
product: vxn-1b
title: "Pan as a matrix destination, Spread as a matrix source, wired by default"
priority: medium
created: 2026-08-07
epic: E036
depends: ["0248", "0262"]
---

## Summary

Voice pan is currently hard-wired: `pan_position(lane) · spread`, computed once
per control block and folded into the summing gains
([bank.rs:623-633](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L623-L633)). It is
the only continuous synthesis parameter in VXN1b that the mod matrix cannot
reach, which is why the instrument has no auto-pan, no envelope-driven placement,
and no way to make a stack move in the image.

This turns the existing hard route into a matrix route:

- **`DestId::Pan`** — a real destination, so LFO/env/velocity/key can drive
  placement.
- **`SourceId::Spread`** — the per-lane spread position as a source.
- **The default patch wires `Spread → Pan` at depth 1.0**, which reproduces
  today's behaviour exactly while making the route visible and editable.

Together they replace a fixed line of DSP with topology, which is what the matrix
is for ([E036](../../epics/open/E036-vxn1b-matrix-engine.md)). Depends on **0248**
having removed the mono fast path — with pan modulated per quantum, no block-rate
"is this mono?" hint can be correct.

## Design

**Dest.** `DestId::Pan = 9`, `N_DESTS` 8 → 9, `DEST_GAIN[Pan] = 1.0` (the dest's
native unit is pan position in `[-1, 1]`, so depth 1 reaches hard). No
`cook_depth` taper — position is already perceptually linear-ish and the useful
range is the whole span, unlike `Pitch`. Names/labels: `"pan"` / `"Pan"` in
[DEST_NAMES / DEST_LABELS](../../vxn-1b/crates/vxn1b-engine/src/matrix.rs#L201-L209).

**Source.** `SourceId::Spread = 11`, `N_SOURCES` 10 → 11, `is_bipolar() = true`
(it swings ±, and the exhaustive match at
[matrix.rs:96](../../vxn-1b/crates/vxn1b-engine/src/matrix.rs#L96) will force the
decision anyway). Value is `pan_position(lane) · spread_param` — i.e. the source
carries the front-panel Spread knob's scaling, not just the raw lane position.

That choice keeps `Spread` a front-panel control rather than demoting it to "slot
3's depth", and makes the default route's depth an honest 1.0. The consequence to
accept: delete the default route and the Spread knob goes inert. That is correct
behaviour for a matrix-routed instrument, and it is visible in the matrix panel
rather than mysterious.

`SourceInputs` gains a `spread_pos: f32` field, filled per lane at
[bank.rs:492](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L492) where the other
per-voice source inputs are already assembled.

**Application + smoothing.** `dests[Pan]` becomes the pan position, replacing the
`pan_position(v) · ctx.spread` product at
[bank.rs:630](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L630). With no route on
the dest the position is 0 and every lane is centred.

A modulated pan cannot stay block-constant — an LFO into Pan would step once per
control block and zipper. It takes the same treatment as PWM and cross-mod
(0208/0242): a slow one-pole per lane in
[mod_smoothing.rs](../../vxn-1b/crates/vxn1b-engine/src/mod_smoothing.rs), snapped
on note-on, advanced per `PITCH_QUANTUM` inside the frame loop where `pw1`/`pw2`
are already re-cooked ([bank.rs:677-689](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L677-L689)),
re-deriving `pan_l[v]`/`pan_r[v]` at each quantum. The hot summing loop at
[bank.rs:799](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L799) keeps reading
`pan_l[v]`/`pan_r[v]` and is untouched. Lanes with no live pan route stay on the
block-start values and pay nothing, exactly as `pwm_active` gates PWM today.

**Law.** Voice pan moves from VXN1's equal-sum (`gl = 1−pos`, `gr = 1+pos`) to
the same unity-centre constant power as 0248: `gl = √2·cos(θ)`, `gr = √2·sin(θ)`,
`θ = (pos+1)·π/4`. Equal-sum is defensible for static unison placement but wrong
for a modulated dest — an LFO auto-pan on an equal-sum law audibly pumps, because
total power rises toward the extremes. Centre stays at unity, so a spread-0 patch
is bit-identical and the parity fork
([parity.rs](../../vxn-1b/crates/vxn1b-engine/tests/parity.rs), which runs
spread 0) still holds. A spread-1 unison patch does change — that is the
deliberate divergence, and it should be listed in ADR 0002's divergences.

`level_comp` (the allocator's stack-width compensation) keeps riding along as a
per-lane block constant.

**Default patch.** [default_patch()](../../vxn-1b/crates/vxn1b-engine/src/matrix.rs#L366)
gains slot 2: `Spread → Pan`, depth 1.0, `Curve::Lin`, no scale source. Slot
budget: 3 of 16 seeded, 13 left for the player.

**Presets.** Existing patches carry no `Spread → Pan` route, so they load
centred. Since the wire format is name-keyed and sparse, the clean fix is a
loader-side seed: when a loaded patch has *no* route into `Pan`, install the
default one in the first free slot. Worth doing — it is ~10 lines and it stops
every existing preset silently losing its spread.

**UI.** The matrix panel builds its dropdowns from the Rust-side vocab
([ui-web/src/lib.rs:243-244](../../vxn-1b/crates/vxn1b-ui-web/src/lib.rs#L243-L244)),
so both new entries appear with no JS change. Confirm the combo fixtures/tests
that assert vocab length are updated.

## Acceptance criteria

- [ ] `DestId::Pan` and `SourceId::Spread` exist, round-trip through `from_u8`,
      and the existing exhaustive `u8` round-trip tests are extended to the new
      counts.
- [ ] `is_bipolar()` for `Spread` is decided in the exhaustive match (bipolar).
- [ ] Engine test: with the default patch, a spread-1 unison chord places lanes
      across the image exactly as the pre-change hard route did, under the new
      law.
- [ ] Engine test: `Spread` param at 0 with the default route → every lane
      centred, L and R identical.
- [ ] Engine test: LFO1 → Pan at depth 1 sweeps a single held voice from hard
      left to hard right over the LFO period, and `gl² + gr²` stays constant
      through the sweep.
- [ ] A pan sweep does not zipper: the per-quantum smoother is exercised, and a
      voice with no pan route takes the block-constant path (assert the
      `pan_active` gate the way the PWM tests assert `pwm_active`).
- [ ] Pan smoother snaps on note-on — a stolen voice must not glide in from the
      previous note's position.
- [ ] `default_patch()` seeds `Spread → Pan @ 1.0` in slot 2; the
      `default_patch_seeds_amp_and_vibrato_only` test is updated (and renamed) to
      assert three seeds and inert slots from 3 on.
- [ ] Loading a patch with no `Pan` route installs the default route in the first
      free slot; a patch that *does* route Pan is left alone.
- [ ] Matrix panel dropdowns list Pan and Spread with correct labels; a route
      built in the UI reaches the engine.
- [ ] Parity fork still passes at spread 0 (unity centre keeps it bit-identical).
- [ ] ADR 0002 divergence list notes the equal-sum → constant-power change.

## Notes

- Depends on **0248** for both the mono fast-path removal and the layer pan the
  voice pan multiplies into. Final placement = voice pan (matrix) × layer pan
  (mixer strip).
- The classic payoffs this unlocks, worth checking by ear in Reaper
  ([[verify-audio-in-reaper]]): LFO1 → Pan for auto-pan, Env1 → Pan for a
  transient that throws left and settles centre, NoteRandom → Pan for a
  keyboard-scattered stack, and Velocity → Pan.
- Out of scope: per-layer stereo width, and a pan-spread control that scales the
  *whole* image after modulation.
- [[vxn1b-two-layer-param-map]]: no new params here — pan comes in through the
  existing slot topology, so the CLAP count is unchanged.

## Close-out (2026-08-11)

- `DestId::Pan` and `SourceId::Spread` exist: `N_DESTS` 8 → 9
  ([matrix.rs:159](../../vxn-1b/crates/vxn1b-engine/src/matrix.rs#L159)),
  `N_SOURCES` 10 → 11
  ([matrix.rs:56](../../vxn-1b/crates/vxn1b-engine/src/matrix.rs#L56)), both in
  the `from_u8` round-trip
  ([matrix.rs:83](../../vxn-1b/crates/vxn1b-engine/src/matrix.rs#L83)).
  `Spread` is decided bipolar in the exhaustive `is_bipolar` match
  ([matrix.rs:106](../../vxn-1b/crates/vxn1b-engine/src/matrix.rs#L106)).
- Source value is `pan_position(lane) · spread_param`, assembled per lane with
  the other per-voice source inputs as `SourceInputs::spread_pos`
  ([bank.rs:491](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L491)) — so the
  front-panel Spread knob keeps its meaning and the default route's depth is an
  honest 1.0.
- Voice pan moved to the same unity-centre constant-power law as 0248
  (`voice_pan_gains`, [bank.rs:867](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L867)),
  replacing VXN1's equal-sum. Centre stays unity, so the spread-0 parity fork is
  bit-identical and `tests/parity.rs` still passes.
- Modulated pan is smoothed per `PITCH_QUANTUM` inside the frame loop, gated by
  `pan_active` exactly as PWM is
  ([mod_smoothing.rs:167](../../vxn-1b/crates/vxn1b-engine/src/mod_smoothing.rs#L167),
  gate at [bank.rs:546](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L546),
  re-cook at [bank.rs:700](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L700));
  a lane with no live pan route stays on block-start values and pays nothing.
  The hot summing loop is untouched.
- Engine coverage, all green:
  `voice_pan_law_is_constant_power_with_unity_centre`
  ([bank.rs:1048](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L1048)),
  `spread_zero_centres_every_lane`
  ([bank.rs:1082](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L1082)),
  `lfo_into_pan_moves_a_centred_voice`
  ([bank.rs:1114](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L1114)),
  `pan_moves_ramp_rather_than_stepping`
  ([bank.rs:1148](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L1148)).
- Matrix panel dropdowns pick both entries up from the Rust vocab with no JS
  change; `matrix-overlay.test.js` green.
- **Two follow-ups deliberately left open:**
  1. **ADR 0002 divergence list** does not yet record the equal-sum →
     constant-power change. Fold into 0213's ADR pass.
  2. **`pan_position` is still an 8-entry per-bank table**
     ([bank.rs:844](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L844)), which is
     only correct while a stack *is* a bank.
     [0266](0266-vxn1b-stack-width-and-voice-mode.md) makes stack width explicit
     and owns replacing it with a stack-relative fan; that ticket carries the
     acceptance criterion.
- Shipped in 3708e9a.
