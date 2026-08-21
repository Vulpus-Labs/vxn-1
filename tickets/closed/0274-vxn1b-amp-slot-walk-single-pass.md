---
id: "0274"
product: vxn-1b
title: "Amp routes are walked twice per block and re-derive the evaluator's arithmetic"
priority: medium
created: 2026-08-21
epic: null
depends: []
---

## Summary

Two functions in [bank.rs](../../vxn-1b/crates/vxn1b-engine/src/bank.rs) iterate
`table.slots` behind the same `dest == Amp && depth != 0` filter:

- [`amp_coeffs`](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L1088) factors the
  Amp dest into `stat + e1·env1 + e2·env2` so the per-frame VCA costs two FMAs.
  Called **per lane, per block** (×16).
- [`amp_envelopes`](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L1125) (added by
  0271) answers "which envelopes are routed to Amp at all", for the voice-lifetime
  predicate. Called **once per block** — but its answer is topology-only, so it
  does not depend on the lane and could ride along with the first walk.

Separately, `amp_coeffs` re-derives the evaluator's per-slot arithmetic for one
destination:

```rust
let scale = match slot.scale_src.idx() {
    Some(sc) => crate::eval::scale_norm(slot.scale_src, sources[sc]),
    None => 1.0,
};
let coeff = slot.dest.cook_depth(slot.depth) * gain * scale;
```

That is `eval_dests`'s inner expression, spelled out a second time. Its own
comment flags the risk — "`cook_depth` is identity for Amp — called for parity
with `eval_dests` so a future tapered dest can't diverge here" — which is a
comment doing a function's job. Add a `scale_src` rule or a depth taper and there
are now two places to change, one of which only the Amp path exercises.

## Design

- Extract the shared per-slot product into
  `eval::slot_gain(slot: &MatrixSlot, sources: &SourceVals) -> f32`, returning
  `cook_depth(depth) · DEST_GAIN[dest] · scale_norm(scale_src)`. Call it from
  both `eval_dests` and `amp_coeffs`, so the taper/scale rules have one home.
- Fold the two `bool`s into `AmpCoeffs` (`holds_env1` / `holds_env2`) and compute
  them in the same walk, then hoist the result: the topology answer is
  lane-independent, so resolve it once per block rather than recomputing inside
  the per-lane loop. Delete `amp_envelopes`.
- Keep 0271's deliberate asymmetry intact and keep its comment with the code: the
  lifetime flags ignore `scale_src` (a route that exists counts as a route, so a
  momentarily-gated VCA cannot make a note un-endable) and include **every**
  curve, where `e1`/`e2` collect only `Curve::Lin` slots.

## Acceptance criteria

- [x] `table.slots` is walked once per block for the Amp destination, not once
      per lane per block plus once more.
- [x] `eval::slot_gain` is the only expression of
      `cook_depth · DEST_GAIN · scale_norm`.
- [x] 0271's lifetime tests still pass unchanged — in particular
      `an_amp_routed_envelope_holds_its_lane_whatever_the_curve` and the
      `scale_src`-at-zero behaviour.
- [x] Default-patch render is bit-identical.

## Notes

Small perf upside (16× fewer slot walks per block) but the reason to do it is the
divergence risk in the duplicated arithmetic, not the cycles.

Overlaps [0273](0273-vxn1b-routing-rules-single-statement.md) in spirit — same
"one statement per rule" theme — but touches disjoint code, so the two can land
in either order.

## Close-out

Landed 2026-08-21. Files touched: `vxn1b-engine/src/{bank.rs, eval.rs}`.

New `AmpRoutes::resolve(table)` scans the slot table **once per block** and
carries both answers: the live Amp routes with their topology gain pre-cooked,
and the `holds_env1`/`holds_env2` lifetime flags. `amp_envelopes` is deleted.
`amp_coeffs` now walks `routes[..n]` — usually one route — instead of all 16
slots for each of 16 lanes, so the per-block slot visits went from 272 to 16
plus the live-route work.

The shared arithmetic became three functions in `eval`:
`slot_topology_gain` (`cook_depth · DEST_GAIN`), `slot_scale` (the `scale_src`
VCA) and `slot_gain` (their product, used by `eval_dests`). `AmpRoutes` hoists
the first and applies the second per lane, so the taper and scale rules have one
definition each. The comment that used to stand in for this ("called for parity
with `eval_dests` so a future tapered dest can't diverge here") is gone with the
duplication it guarded.

0271's asymmetries are preserved and now pinned by
`the_amp_scan_answers_factoring_and_lifetime_together`: the lifetime flags count
every curve and ignore `scale_src`, where `e1`/`e2` collect only `Lin` Env→Amp
slots. All 0271 tests pass unchanged.

Bit-identity confirmed on all four patches.
