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

## Close-out (2026-09-04)

Option **(2)** taken: the swing period is an explicit control. One file changed,
`vxn-3/crates/vxn3-engine/src/grid.rs`.

- **`SwingPeriod`** — new `Copy + Default` enum in `grid.rs`, `Beat` /
  `#[default] Pair` / `Custom(u8)`, tagged `as_u8`/`from_u8` with unknown → the
  default, matching `SwingShape`. `SwingPeriod::subs(n)` answers the period width
  in subdivisions, clamped to `1..=n` so a period can never straddle a beat
  marker. `Swing` gains a `period` field; `Swing::with_period` is the builder.
  `Grid::set_swing` canonicalises the period through its own tag encoding, so
  `Custom(2)` and `Pair` — one feel — compare equal under the derived `PartialEq`,
  the same obligation the marker tail and the dead sub-count overrides carry.
- **`Grid::sub_pos`** — the beat is tiled by periods of `c` subdivisions and `w`
  is applied inside each:

  ```text
  g     = ⌊k / c⌋ · c
  width = min(c, n_b - g)
  sub_pos(b, k) = m[b] + (g + width · w((k - g) / width)) / n_b · (m[b+1] - m[b])
  ```

  With `c = 2` this is the ticket's formula; with `c = n_b` it is 0347's beat-wide
  warp, which is why `Beat` is kept rather than deleted — on a 16ths lane it is
  8th-note swing, a wanted feel rather than a bug. The `k = 0` / `k >= n` early
  returns are untouched, so the exactness contract holds by construction.
- **The odd-`n` rule** is `width = min(c, n_b - g)`: a period that does not divide
  the sub-count leaves a short trailing group, warped across the subdivisions it
  actually has. At `n = 3` with `Pair` that group holds one subdivision, so
  `w(0) = 0` leaves it unswung on its own boundary while the pair before it
  shuffles. No branch, no second rule — odd `n` is a setting like any other, and a
  triplet lane wanting the beat-wide feel asks for `Beat`. It is also what bounds
  the geometry: `u < 1` strictly ⇒ `w(u) < 1` ⇒ each group's last marker lands
  below the next group's first, and the beat's last below `m[b+1]`, for any `c`
  and `n`.
- **`w` is untouched** — same signature, no `n`, no period. Only its doc and the
  knee comment changed, from "half-beat" to "the period's midpoint".
- **Measured, at full swing:** `n = 4` → `0.375, 0.125, 0.375, 0.125` (the on-beat
  8th holds at 0.5, where 0347 dragged it to 0.75); `n = 8` → four
  `0.1875, 0.0625` pairs; `n = 2` → bit-for-bit the shipped values, verified
  against the pre-0365 expression on skewed markers across 64 amounts.
- **Tests** — 38 in `grid` (was 25), all green; `cargo test --workspace` green,
  including the `groove`/`pattern`/`plocks` allocation traps. New: the three gap
  assertions above, `the_beat_period_reproduces_the_pre_ticket_warp`,
  `a_short_trailing_period_is_warped_across_its_own_width` (`n = 3` and `n = 6`),
  `subs_strictly_increase_at_every_swing_period`,
  `the_beat_markers_are_exact_at_every_amount_and_period` — both of those sweeping
  `n ∈ 1..=MAX_SUBS` rather than `SUB_COUNTS`, since the shapes at risk are the
  short-trailing-group ones (`n = 16` with `Custom(5)`, `n = 11` with `Custom(3)`)
  that the shorter list skips — two zero-swing uniform-grid sweeps across all
  periods, `mapping_round_trips_at_every_swing_period`, plus tag round-trip, period
  clamp and canonicalisation tests.
- **`a_width_never_loses_the_subdivision_it_divides_out`** pins the one latent
  constraint the new arithmetic carries. `sub_pos` divides the warped offset out by
  `width` and multiplies it straight back in, and zero-swing bit-exactness needs that
  lossless. It is — for every width `1..=MAX_SUBS`, by exhaustion rather than by an
  argument that scales: the first failing width is 22 (`22 · (15/22) = 14.999…`).
  `MAX_SUBS = 16` is what keeps it true, so raising `MAX_SUBS` past 21 now fails a
  test rather than silently drifting a straight lane by an ULP.
- **Survived unedited:** `straight_warp_is_the_identity_exactly`,
  `warp_is_monotonic_with_fixed_endpoints` (assertions; its `Swing { shape, amount }`
  literal needed `..Swing::default()` for the new field),
  `subs_strictly_increase_at_every_swing_amount`,
  `zero_swing_reproduces_the_uniform_grid_exactly`,
  `non_dyadic_sub_counts_match_the_old_grid_to_one_ulp`,
  `per_beat_sub_override_places_a_triplet`,
  `mapping_round_trips_for_random_positions` (same one-field literal fix),
  `round_trip_survives_a_single_beat_grid`, `zero_swing_subs_are_evenly_spaced`,
  `default_grid_is_the_old_sixteen_step_lane`. Renamed only:
  `mpc_pulls_the_half_beat_late_…` → `mpc_pulls_the_period_midpoint_late_…`, whose
  name and comment described an interval that no longer exists; its assertions on
  `w` are unchanged and it gained one that the period cannot change `w`.
- **`pos_of` and `locate` needed no change** — both scan `sub_pos` itself rather
  than inverting the warp, which is exactly the property 0347 built them for.
  `mapping_round_trips_at_every_swing_period` is the check that it held.
- **Not re-exported from `lib.rs`.** `SwingPeriod` is public at
  `vxn3_engine::grid::SwingPeriod` but is not in the `pub use grid::{…}` list — a
  parallel agent owned `lib.rs` for 0348. One line to add when convenient.
- **e2e:** `cargo run -p vxn3-ui-web --example preview` renders a 47 KB page. That
  static HTML dump is the only browser-visible artifact vxn-3 has — no wasm build,
  no dev server, no JS tests — so no further e2e was possible.
- **Reviewed** by a separate correctness pass that brute-forced the invariants well
  past what the in-repo tests reach: endpoint exactness, zero-swing bit-exactness and
  strict monotonicity over all 18 period spellings × `n ∈ 1..=16` × both shapes × 601
  amounts × six marker geometries, plus all 256 `u8` tag values. No bug found; the
  numerator gap is bounded below by the minimum limb slope (0.5) inside a group *and*
  across a group boundary, short trailing group included. Its findings drove the
  `MAX_SUBS` guard test, the wider `n` sweeps, the non-dyadic arm of
  `the_beat_period_reproduces_the_pre_ticket_warp` (the `Beat` path differs from the
  pre-0365 expression by ≤1 ULP at `n ∈ {3,6,12}`, inside the crate's declared
  tolerance but not the bit-equality the test had asserted), and two doc corrections:
  the canonicalisation claim now says which spellings are folded and why `Beat` is not
  one of them, and the module doc no longer claims long-long-short-short at every
  `n > 2` (at `n = 3` the beat-wide warp is long-medium-short).
- 0354 can now build the swing control against the correct interval, and against a
  period control that exists.
