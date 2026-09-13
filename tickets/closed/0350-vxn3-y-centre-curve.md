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

- [x] Groove carries one Y-centre control point per beat marker; adding or
      removing a marker (0349) adds or removes its control point.
- [x] Curve evaluation is Catmull-Rom with clamped tangents; a property test over
      randomised control points asserts the sampled value never leaves the lane
      bounds.
- [x] A hit's effective Y is `curve(t) + hit.y`, sampled at resolved absolute
      fire time.
- [x] Dragging a hit horizontally across a beat marker changes its effective Y
      **only** by the curve's own continuous variation — no discontinuity at the
      boundary (test samples either side of a marker at decreasing distance and
      asserts the difference converges to zero).
- [x] A flat curve reproduces today's behaviour: effective Y equals `hit.y`.
- [x] Moving a beat marker moves its control point with it; the curve stays
      single-valued in time.
- [x] Evaluation is allocation-free and runs on the audio thread at trig
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

## Close-out (2026-09-13)

Landed. The Y-centre curve is a model plus an evaluator; its editing surface stays
0356.

**Where the control points live — the decision this ticket had to make.** On
`Grid`, as `y_points: [f32; MAX_MARKERS]` beside `markers`. ADR 0007 §8 says they
belong to the *groove*, but the groove type is 0352 and does not exist, so the
question was only ever "where until then" — and `Grid` is not a holding pen, it is
the right answer for three reasons that survive 0352:

- A control point has **no position of its own**; it *is* its marker's. ADR 0007
  §6 wants exactly that ("they move when markers move and need no independent
  position storage"), and storing it next to the thing that defines its position
  makes that true by construction rather than by maintenance. `set_beat_marker`
  and `set_len_beats` need no Y code at all — the contour follows a drag and
  stretches with a length change for free.
- All five marker mutations are already here and already clamped. A parallel array
  elsewhere would have to shadow `set_beat_marker`, `set_len_beats`,
  `insert_beat_marker`, `delete_beat_marker` and `set_n_beats`, and the failure
  mode of missing one is a control point silently belonging to the wrong marker.
- `Grid` is `Copy` and fixed-capacity, so this is 68 bytes and no allocation, and
  it rides the audio-thread swap boundary unchanged.

When 0352 lifts the geometry into a `Groove`, the array goes with it as one more
field of the same struct — a rename, not a redesign, because §8's groove is
precisely "the marker geometry plus this". The doc comment on `Grid::y_point` says
all of the above, so 0352 inherits a decision rather than a surprise.

**The encoding, which is the other judgement call.** ADR 0007 §6's formula is
`effective = curve(t) + hit.y` with `hit.y` an offset. Taken literally that demands
a signed `y` around zero, which would change 0348's stored range, its default, and
what the 0353 faceplate already sends. Instead the offset is carried in lane
coordinates with `Y_CENTRE` as its zero: `effective = curve(t) + (y - Y_CENTRE)`,
clamped to the strip. Same arithmetic, and it buys the AC that matters — with every
control point at the default `Y_CENTRE` the curve is flat and `effective_y(i) ==
hit.y` **bit-for-bit**, so a lane nobody has contoured is unchanged. One constant
now does both jobs (`grid::Y_CENTRE`, re-exported from `sequencer` so
`vxn3-ui-web`'s import is untouched): a control point at `Y_CENTRE` is the flat
curve, a hit's `y` at `Y_CENTRE` is a hit *on* the curve.

That also settles quantise-Y, which looks like it now uses the wrong constant and
does not. `Pattern::quantise_y` pulls toward `Y_CENTRE`, not toward the sampled
curve, because "on the curve" *is* `y == Y_CENTRE` wherever the curve runs. It
needs no knowledge of the contour and no re-running when the contour is edited —
the whole benefit of storing Y relative. The faceplate's existing `Y_CENTRE` lerp
is therefore correct as written rather than provisional.

**The curve — `Grid::y_at(t)`.** Non-uniform Catmull-Rom (beat markers are not
evenly spaced, so the centred difference is the chord secant across both
neighbours) with a **Fritsch–Carlson** tangent limiter: zero at a local extremum or
a flat neighbour, otherwise no steeper than `3 · min(|d_{i-1}|, |d_i|)`. That is
the standard sufficient condition for a monotone piecewise cubic, so each segment
stays between its own two control points and the curve therefore stays in the lane.
Endpoints take the one-sided secant, whose ratio is 1 and so is already inside the
bound. Evaluation is a bounded scan of at most `MAX_BEATS` markers plus a Hermite
segment — `f32`, no allocation, once per resolved trig.

Two exactness details are load-bearing rather than fussy. `hermite` is written as a
displacement from `p0` (`p0 + h01·(p1 - p0) + …`) rather than as the four-term basis
sum, so a segment that should be flat is flat bit-for-bit; the basis sum is flat
only to within rounding, and flat is the case that must be exact.
`Pattern::effective_y` branches on `c == Y_CENTRE` for the same reason — `y + (c -
Y_CENTRE)` cancels only up to rounding.

There is **no mop-up clamp** inside `y_at`, deliberately: a `clamp(0, 1)` there
would make the property test vacuous by flattening an overshoot instead of
preventing it. The residue is that a value can sit a float ulp outside its segment,
and `effective_y` clamps to the lane anyway because the hit's own offset can leave
it regardless.

**Y's destination is velocity**, multiplicatively and centred on `Y_CENTRE`
(`Pattern::y_velocity_scale`, applied in `lane.rs` at resolve time). This is ADR
0006's two-layer rule retained by ADR 0007: `hit.velocity` is the compositional
accent, the contour is the feel scaling it. `y = Y_CENTRE` scales by exactly 1.0
(`/ 0.5` is exact over the whole range), so the default lane fires the velocities
it stores. It goes in `lane.rs` and not `track.rs` because that is where the fire
time exists — by the time a trig reaches the renderer it is a frame offset in a
block it may not have resolved in. A retrig burst is scaled as a whole and its
`vel_end` ramp shapes it within that: a burst is one hit's height on the lane, not
a sub-hit's, so the ramp is not resampled per sub-hit.

Reaching the top of the strip therefore needs headroom in `hit.velocity`; a hit
already at 1.0 saturates on the way up. That is the honest consequence of velocity
being bounded, and it is the one direction the user can always recover, unlike an
additive contour which caps the same way *and* stops scaling quiet hits.

**Marker edits.** Insert gives the new marker the curve's value **at the position
the marker takes**, so a split moves nothing the user can see; the curve is not
bit-identical afterwards, since a new knot changes its neighbours' tangents, but it
still passes through every old control point and through the split point, which is
the strongest form an interpolating spline allows. Delete drops the point with its
marker. A refused insert rolls both arrays back and leaves the grid bit-for-bit as
it was. `set_n_beats` **resamples** the old curve onto the new markers rather than
flattening it — the contour is a shape in time, and re-laying the grid is not a
statement about the feel drawn over it.

### Acceptance criteria

- **Control point per beat marker, added and removed with it** —
  `insert_and_delete_add_and_remove_a_control_point`,
  `a_marker_edit_carries_the_control_points_and_leaves_the_hits_alone`,
  `a_refused_insert_restores_the_control_points`.
- **Clamped Catmull-Rom; property test over randomised control points** —
  `the_curve_never_overshoots_its_control_points`: 400 randomised grids (randomised
  beat counts, lengths, marker positions including neighbours pinned at `MIN_SLOT`,
  a quarter of the points at 0.0 or 1.0), swept at ~300 positions each plus every
  marker. It asserts the *tighter* property — each sample inside its own segment's
  two control points — because lane bounds alone would pass on an implementation
  that clamped the overshoot away.
  `an_unclamped_tangent_would_overshoot_where_this_one_does_not` pins that the
  clamp is not vacuous, by showing the plain centred tangent leaving the lane on
  the same data.
- **`curve(t) + hit.y` at the resolved fire time** —
  `the_effective_y_is_the_curve_at_the_fire_time_plus_the_offset`. Three hits on
  one subdivision marker at different in-slot fractions must read three different
  centres, none of them the marker's; restating the formula would pass whatever it
  sampled.
- **No discontinuity across a beat marker** —
  `a_drag_across_a_beat_marker_does_not_step_in_y` samples either side of every
  interior marker (different beats, different slots) at 20 halvings and asserts the
  gap shrinks with the distance, to below `1e-6`.
  `the_curve_is_continuous_across_every_beat_marker` is the same at grid level.
- **Flat curve reproduces today's behaviour** —
  `the_default_curve_is_flat_at_y_centre_exactly` (`assert_eq!`, not an epsilon),
  `on_a_flat_curve_the_effective_y_is_the_stored_y`, and
  `a_flat_lane_fires_at_the_velocities_it_stores`.
- **A marker drag moves its control point; the curve stays single-valued** —
  `a_marker_drag_carries_its_control_point`, `a_marker_drag_sweeps_the_contour`.
- **Allocation-free, at trig resolution** — `y_curve_resolution_is_allocation_free`
  in `tests/pattern.rs`, under that file's `#[global_allocator]` trap: every lane
  contoured, every hit off both its marker and the curve, so each trig runs a real
  segment lookup and a real cubic. `the_y_contour_changes_what_the_lane_sounds_like`
  is the end-to-end claim that the contour is *heard*.

### Two bugs the review found

1. **`insert_beat_marker` sampled the curve at the requested position, not the
   landed one.** `pos` is only required to be inside the pattern, and the write goes
   through `set_beat_marker`, which clamps into the slot being split — so a caller
   naming a position in another slot got a control point carrying the curve's value
   from wherever it pointed (measured: 0.27 of a lane), exactly inverting the
   inheritance the code is for. Now sampled at the clamped position;
   `a_clamped_insert_inherits_the_curve_where_the_marker_lands` pins it and asserts
   the two positions are still far enough apart to distinguish.
2. **`hermite`'s doc comment had fused with `sane_len`'s**, leaving `sane_len`
   undocumented and `hermite` documented as a pattern-length helper. Reordered.

Also from review: `Pattern::y_centre` now guards an out-of-range index as
`effective_y` does, rather than reading the contour at the pattern origin for a hit
that does not exist; `set_n_beats`'s doc no longer claims the length is always
unchanged (`sane_len` can raise it); and the velocity clamp's comment no longer
reads as a NaN guard, which it is not — a stored NaN velocity still arrives as NaN,
exactly as before this ticket.

### Out of scope, deliberately

- **Curve editing.** No `EngineCommand` for a control point and no faceplate
  change. `Pattern::set_y_point` / `y_point` are the model's door and 0356 is the
  gesture. The faceplate half is untouched: `assets/app.js`, `assets/style.css` and
  `vxn3-ui-web/src/lib.rs` are owned by 0354/0355 in this wave.
- **Per-lane routable Y.** Velocity is the destination, as ADR 0007 §Consequences
  and this ticket's Notes require. No new `flavour` source slot, so ADR 0005's
  matrix is not widened.
- **Persistence.** Patterns are not in the `clap.state` blob yet (ADR 0007
  redefines it and 0348 landed no writer), so there is no format to extend.

Verification: `cargo test --workspace` green (185 `vxn3-engine` lib tests plus the
allocation traps in `tests/{fx,groove,kit,pattern,plocks,faceplate_io}.rs`);
`cargo test -p vxn3-clap` green; `cargo run -p vxn3-xtask --release -- bundle`
bundles. Clippy clean for `vxn3-engine --all-targets`. No browser check — this
ticket is engine-side and the faceplate is not touched. `cargo fmt` was **not**
run: `main` is deliberately not rustfmt-clean and there is no CI gate, so the new
code matches house style by hand.

Unblocks 0356 (dragging the control points).
