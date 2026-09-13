---
id: "0350"
product: vxn-3
title: "Y-centre interpolated curve — control points on beat markers, sampled at fire time"
priority: medium
created: 2026-09-04
epic: E050
depends: ["0347", "0348"]
---

## Summary

Implements [ADR 0007](../../vxn-3/adrs/0007-vxn3-continuous-lane-editor.md) §6.
The groove carries a **Y-centre control point per beat marker**, interpolated
across the region between them and sampled at a hit's absolute time. A hit's
stored `y` (0348) is an offset from that curve, not an absolute lane position.

This is the generalisation of [ADR 0006](../../vxn-3/adrs/0006-vxn3-groove.md)'s
per-slot velocity contour: same idea, continuous instead of stepped, and routed
to whatever the lane's Y destination is rather than fixed to velocity.

## Design

Catmull-Rom between control points, **tangents clamped** so a steep adjacent pair
cannot overshoot outside the lane bounds. Control points sit on beat markers, so
they move when markers move (0349) and need no independent position storage.

Interpolated rather than per-slot is the load-bearing choice, and it is a direct
manipulation requirement rather than an aesthetic one. A per-slot step function
means a hit dragged horizontally across a marker **jumps vertically** — the user
moves X and Y changes. A continuous curve has no discontinuity to cross, which is
what makes it safe to render diamonds relative to the curve at all: dragging sets
offset-from-curve, and editing the curve sweeps the lane's whole contour as one
gesture.

Sampling is at the hit's **absolute** fire time, after 0348 resolves position, so
the curve is indifferent to which slot a hit belongs to.

## Acceptance criteria

- [ ] Groove carries one Y-centre control point per beat marker; adding or
      removing a marker (0349) adds or removes its control point.
- [ ] Curve evaluation is Catmull-Rom with clamped tangents; a property test over
      randomised control points asserts the sampled value never leaves the lane
      bounds.
- [ ] A hit's effective Y is `curve(t) + hit.y`, sampled at resolved absolute
      fire time.
- [ ] Dragging a hit horizontally across a beat marker changes its effective Y
      **only** by the curve's own continuous variation — no discontinuity at the
      boundary (test samples either side of a marker at decreasing distance and
      asserts the difference converges to zero).
- [ ] A flat curve reproduces today's behaviour: effective Y equals `hit.y`.
- [ ] Moving a beat marker moves its control point with it; the curve stays
      single-valued in time.
- [ ] Evaluation is allocation-free and runs on the audio thread at trig
      resolution, not per sample.

## Notes

Y's *destination* is fixed for now — ADR 0007 §Consequences flags per-lane
routable Y as a future ADR and E050 puts it out of scope. Velocity is the sane
default destination, which makes this ticket a superset of ADR 0006's velocity
contour.

Curve **editing** (dragging control points on the faceplate) is 0356. This ticket
is the model and its evaluation.

Clamped tangents matter more than they look: unclamped Catmull-Rom overshoots on
a steep pair, and an overshoot here is a modulation value outside its declared
range being handed to
[`flavour::resolve`](../../vxn-3/crates/vxn3-engine/src/flavour.rs), which clamps
it silently — so the bug presents as "the curve does nothing here", not as an
error.
