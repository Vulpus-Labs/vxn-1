---
id: "0348"
product: vxn-3
title: "Pattern becomes a hit list — relative position over marker geometry"
priority: high
created: 2026-09-04
epic: E050
depends: ["0346", "0347"]
---

## Summary

The pivot ticket of
[E050](../../epics/open/E050-vxn3-continuous-lane-editor.md), implementing
[ADR 0007](../../vxn-3/adrs/0007-vxn3-continuous-lane-editor.md) §4. Replace the
indexed [`Pattern`](../../vxn-3/crates/vxn3-engine/src/sequencer.rs#L151) —
`[Step; 16]` plus `len`, `step_beats` and a `(step, param)` lock table — with a
list of freely-positioned hits over 0347's marker geometry.

Position stops being an array index and becomes a relative coordinate:

```rust
struct Hit { beat: u16, sub: u8, f: f32, nudge: i16, y: f32, rgb: [f32; 3], /* trig attrs */ }
```

```text
t = sub_pos(beat, sub) + f · (sub_pos_next - sub_pos) + nudge
```

## Design

The two-part offset is load-bearing, per ADR 0007 §4. `f ∈ [0,1)` is
proportional, so a hit scales with its slot as swing lengthens it — and `f = 0`
**welds** a hit to its subdivision marker, so a snapped pattern survives every
groove edit with no re-quantise pass. `nudge` is absolute ticks and does *not*
scale, so a deliberate flam survives a swing change unchanged.

`nudge` is clamped to ±½ `MIN_SLOT`. This is what preserves the monotonic
fire-order invariant that bounds 0346's lookahead window: `f ∈ [0,1)` already
keeps a hit inside its own slot, leaving `nudge` as the only term that could
reorder.

`MAX_HITS` replaces `MAX_STEPS` as the per-lane ceiling. Storage stays a
fixed-capacity array; an over-capacity insert drops rather than allocating.

The p-lock table is re-keyed from `(step, param)` to `(hit, param)` — a lock now
belongs to a hit rather than to a grid cell. `Termination::Revert { n }` keeps
its current semantics but `n` now counts **subdivision slots**, which is what
"lane tick" means once the grid is non-uniform; state this in the doc comment on
[`Termination`](../../vxn-3/crates/vxn3-engine/src/sequencer.rs#L136).

**No migration.** vxn-3 is experimental with no user base, so the per-track patch
blob (ADR 0005 / ticket 0179) is **redefined, not converted** — no reader for the
old indexed layout, no dual-path, no compatibility shim. Delete the old
serialisation rather than leaving it behind a version branch.

The version tag survives for one reason: a stale blob left on a developer's own
disk must be **rejected**, not misparsed into the audio engine.
`FLAVOUR_VERSION`-style discipline applies to that check and nothing else.

## Acceptance criteria

- [ ] `Pattern` holds a fixed-capacity hit list plus a marker set from 0347;
      `steps`, `len` and `step_beats` are gone.
- [ ] `step_at` / `lock_at` are replaced by hit-indexed equivalents; p-locks key
      on `(hit, param)`.
- [ ] `f = 0` hits land exactly on their subdivision marker at every swing
      amount — `f64` equality against `sub_pos`, not an epsilon.
- [ ] `nudge` is unscaled by swing: a hit with a fixed `nudge` keeps the same
      absolute offset from its marker as the slot around it lengthens.
- [ ] `nudge` is clamped to ±½ `MIN_SLOT` on every write path.
- [ ] Property test over randomised hit placements, nudges and swing amounts:
      resolved fire times are **strictly non-decreasing** in hit order.
- [ ] 0346's scheduler consumes the hit list; the process callback stays
      allocation-free under `tests/groove.rs`'s trap.
- [ ] The patch blob is redefined with no reader for the old indexed layout; the
      superseded serialisation code is deleted, not branched around.
- [ ] A stale pre-bump blob is rejected with a clear error rather than
      misparsed.
- [ ] `Termination::Revert { n }` counts subdivision slots, documented on the
      type, with a test on a lane whose sub-count varies per beat.

## Notes

Gates every remaining E050 ticket. Y and `rgb` are stored here but not yet
consumed — 0350 and 0351 wire them up. Storing them now avoids a second blob
version bump.

Trig attributes (probability, retrig n/m/curve, velocity) move onto the hit
unchanged; the ADR 0001 §3a split between trig attributes and p-lockable
continuous params is not revisited.

Existing patterns and saved state are expendable — vxn-3 has no user base. Where
a choice arises between preserving something and a cleaner model, take the
cleaner model. The regression bar for this epic lives in 0346, which lands the
risky rewrite against an unchanged data model; by this ticket the reference
render has already done its job.

## Close-out (2026-09-06)

Landed. `Pattern` is now `{ grid: Grid, hits: [Hit; MAX_HITS], n_hits }` — the
`[Step; 16]`, `len` and `step_beats` are gone, and `Step` with them.

**Position.** `Hit { beat, sub, f, nudge, y, rgb, note, velocity, probability,
retrig, locks }`, resolved by `Pattern::fire_beat`:

```text
t = sub_pos(b, k) + f · (sub_pos(b, k+1) - sub_pos(b, k)) + nudge
```

`f <= 0` takes a separate branch returning `p0 + nudge`, so welding is
bit-exact rather than exact-by-arithmetic-luck —
`welded_hits_sit_exactly_on_their_marker_at_every_swing` asserts `f64` equality
against `Grid::sub_pos`, not an epsilon.

**The nudge clamp is in two places, and both are load-bearing.**
`TICKS_PER_BEAT = 30_720` is sized so ±½ `MIN_SLOT` is the exact integer
`MAX_NUDGE_TICKS = 240` (const-asserted). Every write path clamps to that. But
that bound is expressed in *beats*, and a subdivision slot can be shorter than
`MIN_SLOT` is wide, so `fire_beat` clamps the resolved nudge a second time to
±½ of the hit's own slot. Without the second clamp the storage clamp alone does
not imply monotonic fire order, which is the invariant bounding 0346's window.
`resolved_fire_times_are_non_decreasing_in_hit_order` is the property test over
randomised placements, nudges and swing amounts (hand-rolled xorshift32, per
this repo's convention — no proptest dependency).

**P-locks moved onto the hit** rather than being re-keyed in a side table. A
`(hit, param)` side table would still come adrift when the list re-sorts — and
it re-sorts on every insert, every `f`/`nudge` edit and every grid edit —
so the lock array lives in `Hit`. `locks_travel_with_their_hit_through_a_resort`
pins it.

**`Termination::Revert { n }` counts subdivision slots**, documented on the
type, with `revert_counts_subdivision_slots_on_a_varying_sub_count` exercising a
lane whose sub-count changes per beat.

**Scheduler cursor re-anchoring — an extra fix, not in the original ACs.** Both
`next_trig_index` and `next_lock_index` are indices into a numbering the pattern
defines, and the pattern is edited from the audio thread. An edit that changes
the hit count or the grid's slot count rewrites what an already-committed index
means, by a margin proportional to how long the lane has been playing. Each
cursor now records the beat it was committed at and re-anchors when its index
stops naming that beat. Without it, a live grid edit strands the lane silent
with its p-locks frozen for minutes; `a_live_edit_does_not_strand_the_cursors`
and `live_grid_edits_stay_allocation_free_and_keep_the_lane_running` cover it.

**Blob.** `VERSION` 2 → 3 and the check tightened from `> VERSION` to
`!= VERSION`, so a stale v1/v2 blob is rejected rather than half-read into the
audio engine. The old indexed serialisation and
`v1_blob_loads_with_default_flavour_then_upgrades` are deleted, not branched
around. `every_other_version_is_rejected` covers `[0, 1, 2, VERSION+1, 0xFFFF]`.

**Naming.** `lane::Hit` (the emitted event) became `TrigEvent` so the stored
type could take the `Hit` name the ADR uses.

**Faceplate.** Kept working, not rebuilt — that is 0353. The 16-cell grid is now
driven from `Grid::total_subs()`, `toggle_step`/`set_step` map to hit
insert/delete at `(beat, sub, f = 0)`, `SetStepBeats` became a grid-geometry
command, and the playhead publishes a subdivision-slot index from
`Grid::locate`. `EngineCommand` stays `Copy` for the SPSC ring.

`y` and `rgb` are stored and unconsumed, as planned — 0350 and 0351 wire them
up without a second blob bump.

Verification: `cargo test --workspace` green (vxn3-engine 112 lib tests), including
the allocation traps in `tests/{groove,pattern,plocks}.rs`; clippy clean for the
vxn-3 crates; `cargo run -p vxn3-ui-web --example preview` renders a 48 KB page.

Unblocks 0349, 0350, 0351 and 0353.
