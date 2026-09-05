---
id: "0349"
product: vxn-3
title: "Marker edit semantics: drag preserves relative, insert/delete preserves absolute"
priority: medium
created: 2026-09-04
epic: E050
depends: ["0347", "0348"]
---

## Summary

Implements [ADR 0007](../../vxn-3/adrs/0007-vxn3-continuous-lane-editor.md) §5 —
the mutation API for beat markers, and the deliberately **opposite** rules the
two gesture families follow.

- **Drag a marker → preserve relative.** Moving `m[i]` stretches slot `i-1` and
  squashes slot `i` at once, so hits rubber-band in *both* directions from a
  single grab. This is free from 0348's storage: the fractions are unchanged and
  the slot bounds moved underneath them.
- **Insert or delete a marker → preserve absolute.** Each affected hit's
  `(beat, sub, f)` is recomputed from its current absolute time, so splitting or
  merging a slot moves nothing on screen.

Both match what a user expects of the respective gesture. The asymmetry has to be
explicit in the API, not emergent from whichever code path happened to be
written first.

## Design

Drag is a pure write to `m[i]` with the `MIN_SLOT` clamp from 0347 — no hit is
touched, which is what makes undo cheap (one stored value, however many hits
appear to move).

Insert and delete go through 0347's inverse mapping: resolve absolute time for
every hit in the affected span *before* the marker change, apply it, then
re-resolve `(beat, sub, f)` from those times. Delete merges two slots; insert
splits one. Hits at `f = 0` on a deleted marker land at a non-zero `f` in the
merged slot, which is correct — their absolute time is what was preserved, not
their snappedness.

Outer markers are pinned (0347) and so are not draggable, insertable-before or
deletable.

## Acceptance criteria

- [ ] Dragging `m[i]` moves hits in **both** slot `i-1` and slot `i`, in
      proportion, with no hit record written.
- [ ] A marker drag clamps to `(m[i-1] + MIN_SLOT, m[i+1] - MIN_SLOT)`; no path
      can write a marker position that bypasses the clamp.
- [ ] Inserting a beat marker changes no hit's absolute fire time (`f64`
      equality on the resolved times before and after).
- [ ] Deleting a beat marker changes no hit's absolute fire time.
- [ ] Delete-then-insert at the same position is a no-op on fire times; the
      stored `(beat, sub, f)` triples need not round-trip and a test asserts the
      *times* rather than the triples.
- [ ] Outer markers reject drag, delete, and insert-outside.
- [ ] Round-trip property test: randomised marker edits interleaved with
      randomised hit placements never produce out-of-order fire times, a
      degenerate slot, or a NaN.
- [ ] Undo of a marker drag restores one value and all apparent hit positions.

## Notes

The NaN case is the one to guard hardest — a bypassed clamp produces a
zero-width slot, which makes 0347's inverse mapping divide by ~0 and silently
poisons every hit in that slot. It will not fail a unit test unless a test looks
for it explicitly.

Editor gestures (what a mouse drag does, rubber-band feedback) are 0354; this
ticket is the engine-side API those gestures call.
