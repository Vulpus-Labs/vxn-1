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
