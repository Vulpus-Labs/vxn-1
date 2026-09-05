---
id: "0352"
product: vxn-3
title: "Groove object, pool, per-lane assignment, and lock-together on beat markers"
priority: medium
created: 2026-09-04
epic: E050
depends: ["0349", "0350"]
---

## Summary

Implements [ADR 0007](../../vxn-3/adrs/0007-vxn3-continuous-lane-editor.md) §8.
Carries forward [ADR 0006](../../vxn-3/adrs/0006-vxn3-groove.md)'s pooling model
— named, reusable, swappable grooves assigned per lane — with the payload ADR
0007 gives it:

```text
Groove {
  markers,       // beat marker positions (0347)
  sub_counts,    // lane default + per-beat overrides (0347)
  swing,         // the warp (0347)
  y_centre,      // control point per beat marker (0350)
}
```

The pooling survives *better* under relative storage than it did under 0006's
offset table: swapping a groove re-times every hit without touching one of them,
and cannot fight a hand-placed position, because position and feel are no longer
competing for the same field.

## Design

**Assignment is per lane**, which is what preserves ADR 0001 §2 polymeter.

**Lock-together covers beat markers only** — never sub-count, never swing. The
case that pays for the whole feature is a shared beat grid with lane A in swung
16ths and lane B in straight triplets; locking sub-count or swing would destroy
it.

Two things ADR 0007 §8 requires stating outright, because both are the kind of
thing that gets decided by accident:

- **On lock, one lane's positions must win by explicit choice** — "adopt beat
  markers from lane N" — not implicit first-lane-wins.
- **On unlock, lanes keep their current positions** rather than springing back to
  what they had before the lock.

0006's determinism rule is retained unchanged: any randomised humanise derives
from a per-hit deterministic hash of position, never `Math.random`, so loops and
resume reproduce.

Grooves are project/host state, not pattern data — as in 0006.

## Acceptance criteria

- [ ] `Groove` holds markers, sub-counts, swing warp and Y-centre control points;
      lanes hold a pool reference, or none (= straight grid).
- [ ] Swapping a lane's groove re-times its pattern with **zero hit records
      written**, verified by comparing the hit list byte-for-byte before and
      after while asserting the resolved fire times changed.
- [ ] Sharing one groove across two lanes gives them identical marker geometry.
- [ ] Lock-together propagates beat marker edits across locked lanes and leaves
      each lane's sub-count and swing independent — test a swung-16ths lane
      locked to a straight-triplets lane.
- [ ] Lock requires an explicit source lane; there is no implicit winner.
- [ ] Unlock leaves each lane at its current positions.
- [ ] Grooves round-trip through project state; a groove shared by two lanes is
      stored once, not twice.
- [ ] No groove edit can produce a degenerate slot — the 0347 clamp applies to
      pool-level edits too.

## Notes

The failure mode to watch is a groove edit path that writes markers directly
instead of going through 0349's clamped API, which reintroduces the zero-width
slot NaN by the back door.

Groove pool **UI** is 0356. Groove extraction from an existing pattern, and
groove import, are explicitly out of E050's scope.
