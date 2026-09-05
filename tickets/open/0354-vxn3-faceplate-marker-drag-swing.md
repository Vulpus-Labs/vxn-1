---
id: "0354"
product: vxn-3
title: "Faceplate: beat marker drag with two-sided rubber-band feedback, and the swing control"
priority: medium
created: 2026-09-04
epic: E050
depends: ["0349", "0353", "0365"]
---

## Summary

Makes the grid editable on the faceplate — the gestures over 0349's API.
Dragging a beat marker stretches the slot on its left and squashes the slot on
its right **at the same time**, so hits rubber-band in both directions from a
single grab.

That two-sidedness is the part users will not predict, so it needs to be shown:
highlight both adjacent slots during the drag, so it is obvious that the hits to
the left are moving too.

Also the swing control, which redistributes the derived subdivision markers
within each beat (0347's warp).

## Design

Marker drag calls 0349's clamped write; the editor never writes a marker
position directly. Feedback during drag:

- both adjacent slots highlighted,
- the clamp bounds visible as the marker approaches `MIN_SLOT` from either side,
- hits redrawn live from `sub_pos`, not translated by a pixel delta.

Marker **insert** and **delete** use 0349's absolute-preserving paths, so hits
visibly stay put — the opposite of drag, and worth a different affordance
(double-click to insert, right-click to delete) so the two are not conflated.

Swing is one control per lane driving the warp. It is **self-documenting**:
uneven subdivision spacing is visible on the strip, so the user sees the swing
rather than reading a percentage. Show the number too, but the geometry is the
primary readout.

Sub-count is per lane with a per-beat override; the override is how tuplets are
entered, and needs a gesture on the beat rather than a separate mode.

## Acceptance criteria

- [ ] Dragging a beat marker moves hits in both adjacent slots, live, at
      interactive frame rates.
- [ ] Both adjacent slots are visually highlighted for the duration of the drag.
- [ ] The drag clamps at `MIN_SLOT` with visible feedback; no marker can be
      dragged past a neighbour.
- [ ] Insert and delete of a beat marker leave every hit visually stationary.
- [ ] Insert and delete have distinct affordances from drag.
- [ ] Outer markers are visibly non-draggable.
- [ ] The swing control redistributes subdivision markers within each beat, and
      hits at `f = 0` stay welded to their markers throughout the sweep.
- [ ] Per-beat sub-count override is settable from the strip; a beat set to 3
      shows three evenly-spaced subdivisions.
- [ ] Undo of a marker drag restores the marker and all apparent hit positions in
      one step.

## Notes

The welded-hit behaviour under a swing sweep is the demo that sells the model —
worth making it the thing to try first when this lands.

All redraw goes through `sub_pos` (0347). Translating hits by a pixel delta
during a drag will look right and be wrong: it double-applies once the model
updates, and drifts on a non-uniform grid.
