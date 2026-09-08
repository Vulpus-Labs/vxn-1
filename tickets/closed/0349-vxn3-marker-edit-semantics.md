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


## Close-out (2026-09-07)

Landed. ADR 0007 §5's two opposite rules are two named methods on `Pattern`, and
the asymmetry lives in the API rather than in whichever code path was written
first.

**Geometry — `Grid::insert_beat_marker(i, pos) -> Option<usize>` and
`Grid::delete_beat_marker(i) -> bool`.** Insert shifts `markers[]` and
`sub_override[]` up and writes the new position **through `set_beat_marker`**, so
there is exactly one clamped path into the marker array and no second answer to
`MIN_SLOT`. Delete shifts both down. Refusals: the lane already at `MAX_BEATS`; a
non-interior index (`i == 0` or `i > n_beats`) or a `pos` outside the pattern,
either of which would unpin an outer marker; a non-finite `pos`; and a slot too
narrow to yield two of `MIN_SLOT`. A refused insert rolls its own shift back and
leaves the grid bit-for-bit as it was — verified over 3000 randomised grids
during review, and by `a_refused_insert_leaves_the_grid_untouched`.

The split halves inherit the sub-count of the beat they were cut from; a merge
keeps the surviving (left) marker's. That last one loses information by
construction — two beats of different sub-counts merge into one beat, which has
room for one count — so delete-then-insert is a round trip on *fire times*, not
on sub-counts. `insert_and_delete_carry_the_sub_count_overrides` pins both
directions, including the one that does not restore.

`Grid::set_n_beats` stays the *other* answer to a change of beat count — throw
the positions away and re-lay evenly — and its doc says so instead of deferring
to this ticket.

**Hits — `Pattern::drag_beat_marker`, `insert_beat_marker`,
`delete_beat_marker`.** A drag goes through the existing `edit_grid`, the
**relative** door: not one hit record is written, the slot bounds move underneath
the stored fractions, and slot `i-1` stretches while slot `i` squashes from the
same grab. Undo is therefore one stored `f64` — `undoing_a_marker_drag_restores_
every_hit_position` drags away and back and asserts `f64` equality on every fire
time (fuzzed to 3000 randomised trials during review, with nudges and swing).

Insert and delete go through a new **absolute** door,
`edit_grid_preserving_times`: resolve every hit's grid position, apply the edit,
rebuild `(beat, sub, f)` from those positions with `Grid::locate`, then
`canonicalise` exactly as `edit_grid` does. An edit the grid refuses returns
early rather than rebuilding against unchanged geometry.

**Exactness, stated honestly.** A hit in a slot the edit does not reshape keeps
its triple **bit-for-bit** — same markers, same `f64` values, so `locate` returns
the same slot and the fraction it recomputes rounds back to the same `f32`. Those
hits are `f64`-equal, asserted as such (review: 4000 randomised trials, worst
untouched delta exactly 0).

A hit *inside* the split or merged span is genuinely re-derived, and `f` is `f32`
storage — so its time is exact only when the new geometry can name the position
in an `f32` fraction, and otherwise correct to half an `f32` ulp of its slot
(~8 ns at 120 bpm; review measured a worst case of 0.25 ulp over 4000 trials).
**This is the one AC not met to the letter:** "`f64` equality on the resolved
times" is unachievable for a re-derived hit without widening `Hit::f` to `f64`,
which is 0348's storage and shared with two sibling tickets. The bound is per
edit and does not cancel — 400 random marker edits random-walk a hit by ~2.6e-7
beats (0.13 µs at 120 bpm).

The tests keep the two standards apart rather than papering over them.
`marker_edits_preserve_fire_times_over_random_geometry` classifies each hit by
whether its beat was reshaped and holds untouched hits to `==` and reshaped ones
to the bound, over 200 randomised grids (100 inserts, 100 deletes, asserted). The
hand-built dyadic cases are `f64`-equal for *every* hit, reshaped ones included.
`delete_then_insert_at_the_same_place_is_a_no_op_on_fire_times` asserts times and
not triples, per the AC.
`a_hit_welded_to_a_deleted_marker_keeps_its_time_not_its_snap` is the `f = 0`
case: it lands at `f = 0.5` in the merged slot, at exactly the same time.

**The NaN case.** `random_marker_edits_never_produce_a_degenerate_lane`
interleaves randomised hit placements with randomised drags, inserts, deletes,
sub-count and swing changes — 12,000 edits, with NaN, infinite and far
out-of-range positions fed in on purpose — and after *every* one asserts finite
strictly-increasing markers, a positive span for every subdivision slot, a live
`(beat, sub)` and `f ∈ [0, 1)` on every hit, and finite non-decreasing fire times
inside `[0, len]`. `random_marker_edits_never_degenerate_the_grid` is the same
sweep at the `Grid` level.

**Allocation.** `marker_edits_are_allocation_free` in `tests/pattern.rs` drives
200 drag/insert/delete rounds under that file's `#[global_allocator]` trap; the
sandwich keeps its base times in a `[f64; MAX_HITS]` on the stack. Marker edits
have no `EngineCommand` yet — the gestures that will send them are 0354 — so it
runs on a bare `Pattern`.

### Three fixes the adversarial review forced

1. **A marker drag could write a zero-width slot** (AC 2 and AC 7, refuted). The
   `MIN_SLOT` clamp compares an *absolute* quantity against marker positions, so
   past `2^48` beats `m ± MIN_SLOT` is `m`, the clamp parks a dragged marker
   exactly on its neighbour, and `Grid::locate` divides by ~0. Reachable through
   `edit_grid` + `set_len_beats`, both public. Fixed at the source: `sane_len`
   now caps the pattern at **`MAX_LEN_BEATS = 2^16`**, the single gate on the
   magnitude of every position in the grid. The cap sits below both bounds that
   matter — the `MIN_SLOT` one and the tighter `f32`-fraction one at ~`2^18`
   beats, which is what the exactness claim above rests on — and a `const`
   assertion beside the constant makes raising it past either a compile error.
2. **Insert and delete moved *nudged* hits by up to 3.6 ms.** `fire_beat` clamps
   the resolved nudge to ±½ of the hit's own slot (0348), and insert/delete is
   precisely the operation that changes slot width, so preserving the grid part
   alone preserved where a hit hangs and still moved where it fires — three
   orders of magnitude past the `f32` caveat, and a design leak rather than
   rounding. The sandwich now tries a second candidate placement, shifted by the
   change in the resolved nudge, and takes whichever resolves closer to the fire
   time being preserved; the unchanged case is exact and wins by construction, so
   the common path is still one `locate`.
   **Residual, and the second AC deviation:** a hit at the very start of the
   pattern whose slot has just widened would need a negative grid position to
   hold its fire time, and there is none. Its stored ticks then resolve further
   than they used to, by at most ½ `MIN_SLOT` (3.9 ms at 120 bpm). The
   alternative is to rewrite the stored nudge, which ADR 0007 §4 forbids — a
   nudge is absolute and survives a geometry change unscaled — so the fire time
   yields and the bound is what is asserted, swept in
   `a_nudge_at_its_slot_clamp_is_re_expressed_within_half_a_min_slot`. The
   randomised preservation test now carries deliberately over-range nudges;
   without them its `f32` bound was a weaker claim than it looked.
3. **A refused edit rewrote every `(beat, sub, f)`.** Fire times survived, so it
   was cosmetic, but the sandwich ran unconditionally and "very nearly the
   identity" is not what a rejected gesture should do to a hit list. It now
   returns early when the grid compares equal after the edit.

Two review findings are recorded rather than fixed. Merging two slots can leave
two hits sharing one `(beat, sub)` — they keep their distinct absolute times,
which is the point, but the *slot-keyed* verbs (`set`, `clear`, `toggle`) then
reach only the first; `merging_slots_can_stack_hits_that_the_slot_keyed_verbs_
cannot_separate` pins it, `Pattern::canonical` already documented the same effect
for a lane shrink (0348), and the continuous editor addresses hits by index,
where they are two perfectly distinct diamonds. And `insert_beat_marker` returns
the index but not the position taken, since `pos` goes through the drag clamp —
documented on the method, because an undo record wants `beat_marker(i)`
afterwards and not the `pos` it asked for.

Also: the tests compare hits by a `note` tag rather than by index. `canonicalise`
re-sorts, so indexing both sides of an edit is only sound while the property under
test holds — which is exactly the case where a failing test must not be trusted.

Verification: `cargo test --workspace` green (142 `vxn3-engine` lib tests plus the
allocation traps in `tests/{groove,pattern,plocks}.rs`); clippy clean for the
vxn-3 crates; `cargo run -p vxn3-ui-web --example preview` renders a 48 KB page.
`cargo fmt` was **not** run — `main` is deliberately not rustfmt-clean and
running it reformats 18 files in this crate alone; the new code matches house
style by hand.

Unblocks 0354 (the editor gestures that call this API).
