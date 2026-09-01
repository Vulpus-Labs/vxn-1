---
id: "0340"
product: monorepo
title: "Mod matrix curve: one glyph control per curve, opening a 3×3 polarity × shape picker (both synths)"
priority: medium
created: 2026-08-31
epic: null
depends: ["0341"]
---

## Summary

A route's shaping is two orthogonal axes — a [`Polarity`](../../crates/vxn-core-matrix/src/curve.rs)
that maps the source's range and a [`Shape`](../../crates/vxn-core-matrix/src/curve.rs)
that bends the response — and both synths' matrix panels spell them as two
adjacent text pick-lists. That costs two columns per curve (four per row, since
the scale VCA has a bend of its own) and still leaves the player reading
`Bipolar` + `Exp` and imagining what the composition does.

Replace each pair with **one control per curve**: a small button drawing the
resulting mapping as a glyph, which opens a 3×3 picker — polarity down, shape
across — and dismisses on pick.

Mock (interactive, click a glyph):
[ui-mockups/matrix-curve-picker.html](../../ui-mockups/matrix-curve-picker.html).
The nine mappings plotted large, with formulae and native input ranges:
[ui-mockups/matrix-curve-grid.html](../../ui-mockups/matrix-curve-grid.html)
(regenerate with `python3 ui-mockups/gen-curve-grid.py`).

The panels this lands in:

- vxn-1b — [matrix.js:268-275](../../vxn-1b/crates/vxn1b-ui-web/assets/panels/matrix.js#L268-L275),
  row grid at [faceplate.css:421-429](../../vxn-1b/crates/vxn1b-ui-web/assets/faceplate.css#L421-L429)
  (`80px` polarity + `72px` shape, twice).
- vxn-2 — [mod-matrix.js:331-342](../../vxn-2/crates/vxn2-ui-web/assets/panels/mod-matrix.js#L331-L342),
  row grid at [style.css:1008](../../vxn-2/crates/vxn2-ui-web/assets/style.css#L1008)
  (`72px` + `64px`).

Both already receive the vocabulary as `polarities` / `shapes` id-label lists
([vxn1b-ui-web/src/lib.rs:314-315](../../vxn-1b/crates/vxn1b-ui-web/src/lib.rs#L314-L315)),
so nothing new crosses the bridge — the picker is a different presentation of
the two values the panel already edits, and each axis keeps its own edit
opcode.

## Design

**The glyph.** An SVG polyline of `y = bend(shape, polarity(v))` over
`v ∈ [−1, 1]`, drawn from the same arithmetic the audio thread runs, so the
picture cannot drift from the sound. At row scale (38×22 in the mock, replacing
two columns totalling 152px in vxn-1b) the axis cross is dropped — with it, all
nine read as hash marks — leaving a faint zero line, the curve, and the button's
own border. The picker's cells are large enough to carry the full frame: both
axes, a dotted identity diagonal for reference, and a shaded band marking the
source range that polarity expects.

**Polarity order is None, Abs, Bipolar** — resting state first, then the two
that reshape it. This is **display order only**. The enum's discriminants are
pinned by `curve_code = polarity × 3 + shape`, whose stride keeps the four
pre-split preset spellings (`0 = lin`, `1 = exp`, `2 = log`, `3 = bipolar`)
meaning what they always meant, in both synths; renumbering silently remaps
every saved route. Carry a display-order table (`[0, 2, 1]`) in the panel.

**`Direct` is renamed `None`** — three separately-scoped edits, not one:

1. `POLARITY_LABELS[0]`, `"Direct"` → `"None"`. Display text; safe.
2. `POLARITY_NAMES[0]`, the wire name `"direct"`. vxn-1b's preset TOMLs spell
   it, so the reader needs an alias accepting both. `CURVE_NAMES` elides the
   polarity for codes 0–2 and is untouched.
3. The variant `Polarity::Direct` → `Polarity::None`. Mechanical across both
   synths, but a `use Polarity::*` would then collide with `Option::None`, so
   call sites need qualified paths — [golden.rs:85](../../crates/vxn-core-matrix/src/golden.rs#L85)
   already imports the variants by name.

None of the three touches the arithmetic; the null test and render hashes must
not move.

**Where the code lives.** The glyph renderer and the picker are one primitive
used by two panels, so they belong in
[crates/vxn-core-ui-web/assets/](../../crates/vxn-core-ui-web/assets/) beside
`value-pop.js` and `wire-drag.js`, not copied into each synth — the same
duplication [E049](../../epics/closed/E049-shared-matrix-routing.md) exists to
undo on the DSP side. Each panel supplies the vocab and an edit callback.

**Build it from DOM elements, never a native `<select>`.** Both synths already
hand-roll their pick-lists for this reason: an NSMenu steals webview
first-responder under macOS/WKWebView (see the comment above
[buildSelect](../../vxn-2/crates/vxn2-ui-web/assets/panels/mod-matrix.js#L85)).
The picker is body-attached and `position: fixed` so the overlay's scroll
container can't clip it, same as `.vxn-mm-combo-pop`.

**Both curves get the full 3×3.** The scale VCA is shape-only today, which is
what [0341](0341-scale-vca-polarity-axis.md) fixes; that ticket lands first and
this one is then pure UI, with the same picker driving both positions and no
special case for the scale column.

## Acceptance criteria

- [ ] Each matrix row in **both** synths shows one glyph button per curve
      (route curve, scale curve) in place of the polarity/shape pick-lists;
      the row grid loses a column per curve and no row overflows its panel.
- [ ] Clicking a glyph opens the picker anchored to that button, with the
      current selection marked; picking an option applies it and closes the
      picker; Esc, an outside click, and a window resize all cancel without
      editing.
- [ ] Picker rows read **None / Abs / Bipolar**, columns **Lin / Exp / Log**,
      while `Polarity`'s discriminants and `curve_code` are unchanged — an
      existing preset loads with the same route shaping it had before.
- [ ] Glyph paths are computed from `vxn_core_matrix::curve`'s own arithmetic
      (one shared renderer in `vxn-core-ui-web/assets/`, used by both panels).
- [ ] `"Direct"` no longer appears in any UI surface; a preset written with the
      old `direct` wire name still loads.
- [ ] The scale-curve control offers the same nine options as the route
      curve, driving the scale polarity and scale shape edits from 0341.
- [ ] A vitest covering the primitive: glyph for a `(polarity, shape)` pair,
      open → pick → correct edit emitted → picker closed, and cancel paths
      emitting nothing. vxn-1b's
      [matrix-combo.test.js](../../vxn-1b/crates/vxn1b-ui-web/assets/__tests__/matrix-combo.test.js)
      and [matrix-overlay.test.js](../../vxn-1b/crates/vxn1b-ui-web/assets/__tests__/matrix-overlay.test.js)
      still pass.
- [ ] Checked in a DAW on both synths — the picker is not clipped by the
      overlay, and first click lands (the WKWebView first-responder bug).

## Notes

- Pure UI: no DSP change, so no null test is owed. The `Direct` → `None` rename
  reaches engine crates but only as names.
- Blocked on [0341](0341-scale-vca-polarity-axis.md) only for the scale
  column. The route-curve half could ship first if that is ever useful, but
  shipping a 3×3 picker beside a 1×3 one reads as a bug, so keep them together.
- Independent of [E049](../../epics/closed/E049-shared-matrix-routing.md), which
  is behaviour-preserving DSP extraction. It builds on
  [0330](../../tickets/closed/0330-share-curve-vocabulary.md) (closed) having
  already made the vocabulary shared, so the label and wire-name edits happen
  once rather than per synth. No ordering constraint against the open E049
  tickets — different files.
- Fits [E008](../../epics/open/E008-js-reusable-primitives.md)'s remit if that
  epic wants to claim it; filed standalone rather than assumed into it.
- Out of scope: the flat `curve` pick-list vxn-2 still offers in preset files,
  the depth fader, and any change to what a destination does with its total.
- Glyph legibility at row scale is the risk. `None·Lin` and `Bipolar·Lin` are
  both straight lines, distinguished only by slope and offset once the axis
  cross is gone; if they don't separate on a real screen, the fallback is a
  one-or-two-character overlay on the glyph rather than restoring a text
  column.
