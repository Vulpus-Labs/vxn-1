---
id: "0353"
product: vxn-3
title: "Faceplate: continuous lane strip with draggable diamond hits, snap and quantise"
priority: medium
created: 2026-09-04
epic: E050
depends: ["0348"]
---

## Summary

The first faceplate ticket of
[E050](../../epics/open/E050-vxn3-continuous-lane-editor.md), implementing
[ADR 0007](../../vxn-3/adrs/0007-vxn3-continuous-lane-editor.md) §1. Replaces the
step-grid lane in
[app.js](../../vxn-3/crates/vxn3-ui-web/assets/app.js) with a continuous
rectangular strip per track: **X is time, Y is a modulation value**, and hits are
draggable diamonds placed freely in it.

## Design

The strip draws the marker geometry from 0347 — beat markers heavy, subdivision
markers thin and dim — and hits as diamonds at their resolved positions. The grid
is **drawn, not stored into**; a diamond's position is 0348's `(beat, sub, f,
nudge)` plus `y`, resolved for display.

**Snap and quantise are editor verbs, not storage constraints** (ADR 0007 §1). A
quantised hit is one whose stored `f` happens to be zero; nothing in the data
model knows the difference.

- **Snap** is a toggle applied during drag — the diamond lands on the nearest
  subdivision marker with `f = 0`.
- **Quantise** is applied after the fact to already-placed hits, and gets two
  independent verbs: quantise **X** to the nearest subdivision marker, and
  quantise **Y** to the groove centre curve. Partial quantise lerps `f` toward 0
  or 1 (whichever is nearer) and decays `nudge` by the same amount.

Snap targets are exactly the subdivision markers, beat markers included (0347).

## Acceptance criteria

- [ ] Each track renders as a continuous strip with beat markers visually
      distinct from subdivision markers.
- [ ] Hits render as diamonds and drag freely in X and Y within the strip.
- [ ] Snap toggles; with snap on, a dropped diamond stores `f = 0, nudge = 0` and
      sits exactly on a subdivision marker.
- [ ] Quantise-X and quantise-Y are separate commands and can be applied
      independently to a selection.
- [ ] Partial quantise lerps `f` toward the nearest marker and decays `nudge`
      proportionally.
- [ ] Dragging a hit across a beat marker updates its `(beat, sub)` and
      recomputes `f`; the hit does not visually jump in X.
- [ ] Playhead renders against the strip and tracks the swung grid.
- [ ] Hit add and delete work in the strip; the `MAX_HITS` ceiling is enforced in
      the editor with visible feedback rather than a silent drop.

## Notes

Depends only on 0348. Marker dragging is 0354, the palette is 0355, curve editing
is 0356 — this ticket ships the strip with a static grid, which is enough to
place and hear freely-positioned hits.

Y renders relative to the groove's centre curve once 0350/0356 land; until then a
flat curve makes that reduce to absolute-in-lane, so no rework is needed to
sequence it this way.

Watch the swung grid: subdivision markers are unevenly spaced by construction, so
any pixel-per-step assumption inherited from the old step grid is a bug. Position
comes from `sub_pos`, never from multiplication.
