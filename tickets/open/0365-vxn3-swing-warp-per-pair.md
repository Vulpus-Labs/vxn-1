---
id: "0365"
product: vxn-3
title: "Swing warp applies per pair, not per beat — classic shuffle at n > 2"
priority: high
created: 2026-09-04
epic: E050
depends: ["0347"]
---

## Summary

Corrective ticket of [E050](../../epics/open/E050-vxn3-continuous-lane-editor.md),
implementing the
[ADR 0007 Amendment](../../vxn-3/adrs/0007-vxn3-continuous-lane-editor.md#amendment-2026-09-04--the-warp-applies-per-pair-not-per-beat)
of 2026-09-04.

[ADR 0007](../../vxn-3/adrs/0007-vxn3-continuous-lane-editor.md) §3 glossed
classic MPC swing as *"a piecewise-linear pull on the odd subdivisions … is one
`w`"*, and [0347](0347-vxn3-marker-geometry-swing-warp.md) shipped the literal
reading — `w` applied across the whole beat, in
[`Grid::sub_pos`](../../vxn-3/crates/vxn3-engine/src/grid.rs). That is exact
shuffle at `n = 2` and a **different rhythm** at every other sub-count.

Measured on the shipped grid, one beat at full swing:

| `n` | shipped gaps (beat interval) | classic shuffle |
|----:|------------------------------|-----------------|
| 2   | `0.75, 0.25`                 | `0.75, 0.25` — exact |
| 4   | `0.375, 0.375, 0.125, 0.125` | `0.375, 0.125, 0.375, 0.125` |
| 3   | `0.5, 0.333, 0.167`          | n/a |

Shuffle is long-short-long-short; the beat-wide warp gives
long-long-short-short — the beat back-loaded, not swung. The 16ths land at
`0, 0.375, 0.75, 0.875`: subs 1 and 3 sit exactly where shuffle wants them, and
sub 2 — the on-beat 8th — is dragged to 0.75 instead of holding at 0.5.

## Design

Structural, not a tuning error. Shuffle delays the second of each **pair**, a
pair is `2/n` of a beat, so pulling only the odd subs needs `n/2` knees. A
two-knee `w` is exact 16th shuffle and gives `w(0.5) = 0.5` — no swing at all —
at `n = 2`. **Any fixed `w(u)` is classic swing for exactly one `n`.**

Keep `w` exactly as ADR 0007 §3 defines it — monotonic on `[0,1]`, `w(0) = 0`,
`w(1) = 1`, no `n` argument. Change only the interval it is applied to:

```text
p = k / 2            (pair index)
j = k % 2            (position within the pair)
sub_pos(b, k) = m[b] + (p + w(j / 2)) · (2 / n_b) · (m[b+1] - m[b])
```

`n_b` enters only as the pair width, which `Grid` already holds. Sub markers stay
derived, `w` stays sub-count-agnostic, monotonicity and fixed endpoints are
untouched. At `n = 2` this collapses to today's behaviour.

**Odd `n` needs an explicit answer** — 3 and 6 do not pair evenly. Two options,
and this ticket must pick one rather than let it fall out of the arithmetic:

1. Fall back to the beat interval for odd `n`. Smallest change; leaves two rules
   in one function.
2. Make the swing **period** an explicit control — beat / pair / custom — with
   pair as the default for even `n`. Wider, but it also buys 8th-note swing on a
   16ths lane as a first-class option rather than an accident, and it makes the
   odd-`n` case a setting rather than a special case.

(2) is preferred. It widens the swing control [0354](0354-vxn3-faceplate-marker-drag-swing.md)
has to build, which is why this lands first.

The `k = 0` and `k >= n` exactness contract from 0347 must survive: `sub_pos`
returns the beat marker and the *next* beat marker respectively, bit-for-bit, or
`locate` can land a position in the wrong beat.

## Acceptance criteria

- [ ] At `n = 4` and full swing, one beat's sub gaps are `0.375, 0.125, 0.375,
      0.125` — long-short-long-short, asserted as exact values.
- [ ] At `n = 2` the shipped behaviour is unchanged, bit-for-bit against the
      pre-ticket grid.
- [ ] At `n = 8` the pattern is four long-short pairs, not two halves.
- [ ] Zero swing reproduces the uniform grid at every `n`, to the same standard
      0347 met (`f64` equality for dyadic `n`, 1 ULP for `n ∈ {3,6,12}`).
- [ ] Sub markers stay strictly increasing at every swing amount for every
      `n ∈ {1,2,3,4,6,8}` — 0347's property test extended, not replaced.
- [ ] `w` still takes no `n` argument; its monotonicity and fixed-endpoint
      property tests pass unchanged.
- [ ] `sub_pos(b, 0)` returns `m[b]` and `sub_pos(b, k >= n)` returns `m[b+1]`,
      both exactly, at every swing amount and period.
- [ ] The odd-`n` rule is implemented, documented on the type, and tested at
      `n = 3` and `n = 6`.
- [ ] Forward/inverse round-trip property test still passes (0347's).
- [ ] No allocation in any query path.

## Notes

Depends on 0347, which is landed (`9cd28e6`, `89272d9`). Must land **before**
[0354](0354-vxn3-faceplate-marker-drag-swing.md): the swing control's geometry
*is* its readout per that ticket, so building it against the beat-interval warp
would ship a control that visibly does the wrong thing.

Independent of [0346](0346-vxn3-continuous-lookahead-scheduler.md) and of
[0348](0348-vxn3-pattern-as-hit-list.md) — the scheduler consumes `sub_pos`
without caring what interval it warps over. Can run in parallel with 0348.

The failure mode to watch: this changes what an existing swung lane sounds like.
That is the point, and vxn-3 has no user base (ADR 0007 §Consequences), so no
migration — but a swing amount tuned against the old warp will not mean the same
thing, and 0347's own tests encode the old positions. Expect to rewrite those
assertions rather than preserve them; the invariant tests (monotonic, increasing,
round-trip, exactness at the markers) are the ones that must survive unedited.
