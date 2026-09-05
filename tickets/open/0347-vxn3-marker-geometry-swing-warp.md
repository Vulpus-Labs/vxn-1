---
id: "0347"
product: vxn-3
title: "Marker geometry: beat markers, derived subdivision markers, swing as a warp"
priority: high
created: 2026-09-04
epic: E050
depends: []
---

## Summary

Second ticket of [E050](../../epics/open/E050-vxn3-continuous-lane-editor.md),
implementing [ADR 0007](../../vxn-3/adrs/0007-vxn3-continuous-lane-editor.md)
§2–§3. A new `grid` module in `vxn3-engine` owning the lane's timing geometry:
stored **beat markers**, derived **subdivision markers**, and the swing warp that
positions the latter.

Pure data and math with no scheduler or UI coupling, so it lands independently of
0346 and is exhaustively unit-testable.

## Design

```text
sub_pos(b, k) = m[b] + w(k / n_b) · (m[b+1] - m[b])
```

- `m` — stored beat marker positions in beats, strictly increasing, outer
  markers pinned to the pattern bounds.
- `n_b` — subdivision count for beat `b`: a lane default with a per-beat
  override. `n_b = 3` on one beat inside a 16ths lane is how tuplets are
  expressed; there is no separate tuplet concept.
- `w` — the swing warp, monotonic on `[0,1]` with `w(0) = 0`, `w(1) = 1`.

Sub markers are **never stored**. Beat marker `k = 0` is a sub marker, so the
snap-target set is exactly the sub markers.

`w` is chosen as a warp rather than a per-position offset table specifically so
it generalises over `n` — one swing control stays meaningful whatever a beat's
sub-count, and `n = 3` needs no special case. Ship at least classic
piecewise-linear MPC swing (pull the odd subdivisions late); the enum is
extensible behind a `u8` tag.

`MIN_SLOT` is a hard constant here, not a UI concern: a zero-width slot makes
0348's position fraction unresolvable and its inverse mapping divide by ~0. Every
mutation path goes through the clamp.

## Acceptance criteria

- [ ] `grid` module exposes `sub_pos(beat, k)`, the sub marker count for a beat,
      and forward/inverse mapping between a beat position and `(beat, sub,
      fraction)`.
- [ ] Beat markers are strictly increasing by construction; a mutation that would
      violate this clamps to `MIN_SLOT` rather than being rejected or accepted.
- [ ] Swing warp is monotonic with fixed endpoints; a property test asserts
      `w` monotonic and `w(0) = 0, w(1) = 1` across the full swing range.
- [ ] Sub marker positions are correct for `n ∈ {1, 2, 3, 4, 6, 8}` at zero swing
      (evenly spaced) and stay strictly increasing at every swing amount for
      every `n`.
- [ ] Zero swing with a straight marker set reproduces the current uniform grid
      positions exactly — `sub_pos` agrees with `i · step_beats` to `f64`
      equality.
- [ ] Per-beat sub-count override works: a single `n = 3` beat inside an `n = 4`
      lane places three evenly-spaced subs in that beat and four elsewhere.
- [ ] Forward-then-inverse mapping round-trips for randomised positions inside
      the pattern bounds (property test).
- [ ] No allocation in any query path.

## Notes

> **The swing half of this ticket is corrected by
> [0365](0365-vxn3-swing-warp-per-pair.md).** As shipped, `w` is applied across
> the whole beat, per the literal reading of ADR 0007 §3 — exact shuffle at
> `n = 2`, and long-long-short-short rather than long-short-long-short at
> `n = 4`. The ADR is amended and 0365 moves the warp to the pair interval. Do
> not close this ticket believing the "one swing control, meaningful at any
> sub-count" goal is met; the geometry and its invariants are, the swing feel is
> not.

Parallel with 0346 — nothing here touches
[lane.rs](../../vxn-3/crates/vxn3-engine/src/lane.rs). 0348 consumes both.

Marker *editing* semantics (drag rubber-bands hits, insert/delete preserves
absolute time) are 0349; this ticket owns the geometry and its invariants only.

`step_beats` on
[`Pattern`](../../vxn-3/crates/vxn3-engine/src/sequencer.rs#L151) is superseded by
this module but stays until 0348 removes it — the polymeter behaviour it provides
(ADR 0001 §2) is preserved by per-lane marker sets, not lost.
