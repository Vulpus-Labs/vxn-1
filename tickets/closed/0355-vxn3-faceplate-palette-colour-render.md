---
id: "0355"
product: vxn-3
title: "Faceplate: three-arc palette selector and the colour render rules"
priority: medium
created: 2026-09-04
epic: E050
depends: ["0351", "0353"]
---

## Summary

The user-facing half of
[ADR 0007](../../vxn-3/adrs/0007-vxn3-continuous-lane-editor.md) §7. Shift-click
on a diamond blooms **three 120° arc sliders** around it, each tinted its
channel, dragged independently — driving the three macro slots wired up in 0351.

Plus the render rules, which are correctness requirements here rather than
polish, because in this design **colour carries data**.

## Design

### Why arcs and not a colour picker

Three degrees of freedom do not fit in two dimensions; any widget must add a
second control, a mode, or lose a channel. And these values are modulation
sources — the user must be able to hit `R = 1.0, G = 0, B = 0.5` on purpose.

That rules out every hue-based widget (HSV wheel, Maxwell triangle, corner-on RGB
cube): moving one control changes two or three channels, so no macro slot is
independently addressable. Rejected in the ADR with that reasoning; do not
reintroduce one as a "nicer" alternative.

Three arcs are compact, in-context, occlude no lane, and are exactly orthogonal.
Readouts are normalised `0.00–1.00` — what the matrix wants, not `0–255`. A
three-bar numeric panel is the precise-entry fallback behind a secondary gesture.

### Render rules

- **`R = G = B = 0` is a legitimate, useful value and an invisible diamond.**
  Display clamps to a minimum luminance and strokes with fixed contrast; the raw
  value still goes to the macros. **Render and value are decoupled** — the
  luminance floor must not leak into the value path.
- **Red/green is the worst possible pair to make load-bearing.** A redundant
  non-colour channel — notch rotation, or a three-segment ring on the diamond
  edge — is required, not optional. Without it the lane is unreadable for a
  red/green-deficient user.
- A swatch presets row makes a tuned triple reusable as a macro, which also
  gives users a way to work without discriminating fine colour differences.

### Framing

Present the three values as **macro slots A/B/C with colour as their readout**,
not as "pick a colour". Same data, but it stops users fighting to make a
good-looking pattern and getting macro values they did not intend.

## Acceptance criteria

- [ ] Shift-click on a diamond opens the three-arc selector in place, without a
      popover that occludes the lane.
- [ ] Each arc moves **exactly one** channel; the other two are unchanged to
      `f32` equality.
- [ ] Readouts are normalised `0.00–1.00`.
- [ ] A numeric three-field entry panel is reachable and sets the same values.
- [ ] A hit at `rgb = [0, 0, 0]` is clearly visible on the strip **and** sends
      zero to all three macro slots — assert both in the same test.
- [ ] The luminance floor exists only in the render path; a value-path assertion
      confirms the raw channel values are untouched by it.
- [ ] A redundant non-colour channel encodes the same information and is present
      in every render path.
- [ ] Swatch presets can be saved and applied to a selection.
- [ ] Arc labels/tooltips name the macro slot and its bound param, not "red".

## Notes

Depends on 0351 for the value path — without it this widget edits nothing
audible.

The accessibility redundancy is the criterion most likely to be quietly dropped
as polish. It is not: with colour as the only encoding, a red/green-deficient
user cannot read the pattern at all.

The bound param name in the tooltip comes from the flavour's binding table
([`flavour.rs`](../../vxn-3/crates/vxn3-engine/src/flavour.rs)) and so changes
with the flavour — same flavour-aware dispatch discipline as `value_to_text`
(ticket 0172).

## Close-out (2026-09-12)

- **The palette's value path.** Two new hit-keyed verbs,
  [`EngineCommand::SetHitColour` / `ClearHitColour`](../../vxn-3/crates/vxn3-engine/src/io.rs#L82),
  routed in `EngineCommand::track` and applied through the one
  `apply_pattern_command` both the model and the audio copy run. Separate verbs on
  purpose: black drives all three slots to zero, no colour hands them back.
  Opcodes `set_hit_colour` / `clear_hit_colour` in
  [`parse_custom_ui`](../../vxn-3/crates/vxn3-ui-web/src/lib.rs#L427), with
  `rgb_at` clamping into the normalised `0.00–1.00` domain — below it lies
  `NO_COLOUR`, a different meaning rather than a worse value. Tests:
  `io::tests::the_colour_verbs_carry_raw_channels_into_the_model`,
  `faceplate_io::the_colour_verbs_cross_to_the_audio_thread_unaltered`,
  `tests::parses_the_palette_vocabulary`.
- **Three arcs, in place.** Shift-click on a diamond blooms three 120° arcs
  (8° apart, one per macro slot) hung off the track row — the strip clips its own
  contents — in [`openPalette`](../../vxn-3/crates/vxn3-ui-web/assets/app.js#L935).
  Shift-clicking the same diamond takes the gesture back, which is also how a hit
  leaves a multi-selection. Pinned by
  `tests::the_palette_opens_in_place_and_occludes_no_lane`.
- **Exactly one channel per arc.** `setChannel` copies the triple and writes one
  index; nothing recomputes the other two, so they hold to the bit. Asserted over
  three successive drags in
  `tests::an_arc_moves_one_macro_slot_and_leaves_the_others_alone` (`to_bits`
  equality) and structurally over the shipped JS.
- **Black is visible *and* sends zero**, both in
  `tests::black_is_drawn_above_the_floor_and_still_sends_zero`: the opcode →
  command → model → `colour_override` path yields `[0, 0, 0]`, while the page
  floors *display* luminance from
  [`MIN_DISPLAY_LUMA`](../../vxn-3/crates/vxn3-ui-web/src/lib.rs#L72), shipped in
  the config. The dark end is compressed onto `[MIN_LUMA, 0.5]` rather than
  clamped, so two dark vectors still draw differently.
- **The floor is render-only**, and that is asserted rather than asserted-to-be:
  `tests::the_luminance_floor_never_reaches_the_value_path` extracts the bodies of
  `sendColour` / `sendClearColour` / `setChannel` from the shipped `app.js` and
  requires that none of them reaches `displayRgb` or `MIN_LUMA`, that the floor has
  one implementation and one call site, and that `* 255` appears only where a CSS
  colour string is written.
- **The redundant non-colour channel.** Every coloured diamond wears a
  three-segment ring — one segment per slot, length `2px + value`, on three fixed
  edges filling clockwise from the top vertex, in fixed-contrast strokes; the
  fourth edge stays bare to mark where the ring starts. One painter,
  `paintHitColour`, and
  `tests::the_redundant_ring_is_in_every_render_path` requires every render path
  (`renderHits`, `renderPaletteBar`, `renderSwatches`) to go through it. The
  selection ring takes the accent on a coloured hit so it cannot wash the segments
  out.
- **Readouts, numeric entry and swatches** live in a bar above the rack, so
  nothing that carries data is ever covered: three `0.00–1.00` number fields that
  drive the same `setChannel` the arcs do, a swatch row applied to the whole
  selection, Save, and None. `tests::the_numeric_panel_sets_the_same_values_normalised`,
  `tests::swatches_are_saved_and_applied_to_a_selection`.
- **Framing.** Arc tooltips and bar labels read `macro A · <bound param>`,
  resolved through the lane's assigned voice's binding table (`macroLabel`), so the
  name follows the flavour — the `value_to_text` discipline of 0172. No control in
  the page names a channel by colour: `tests::the_palette_names_macro_slots_not_colours`.
- Verified by driving the shipped `app.js` under a throwaway DOM shim (39 checks:
  arc drags, numeric entry, swatch apply/save, uncolour, every palette-close path).
  `cargo test --workspace` green (105 suites), `vxn3-xtask bundle` builds.
  **Not verified visually — there is no browser or screenshot tool in this
  environment; the arc proportions, the ring's legibility at 11px and the floor's
  contrast against the strip still want a human eye.**
- Out of scope, deliberately: swatches are in-page only (a swatch is a working
  convenience, not part of the pattern, and the page has no preferences store); the
  arcs and numeric fields edit the one focused hit while the swatch row is the bulk
  verb; and while a palette is open its grab band cannot be clicked through to a
  diamond underneath it.
