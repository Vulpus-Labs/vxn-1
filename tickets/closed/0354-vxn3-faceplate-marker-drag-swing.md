---
id: "0354"
product: vxn-3
title: "Faceplate: beat marker drag with two-sided rubber-band feedback, and the swing control"
priority: medium
created: 2026-09-04
epic: E050
depends: ["0349", "0353", "0365"]
---

## Summary

Makes the grid editable on the faceplate — the gestures over 0349's API.
Dragging a beat marker stretches the slot on its left and squashes the slot on
its right **at the same time**, so hits rubber-band in both directions from a
single grab.

That two-sidedness is the part users will not predict, so it needs to be shown:
highlight both adjacent slots during the drag, so it is obvious that the hits to
the left are moving too.

Also the swing control, which redistributes the derived subdivision markers
within each beat (0347's warp).

## Design

Marker drag calls 0349's clamped write; the editor never writes a marker
position directly. Feedback during drag:

- both adjacent slots highlighted,
- the clamp bounds visible as the marker approaches `MIN_SLOT` from either side,
- hits redrawn live from `sub_pos`, not translated by a pixel delta.

Marker **insert** and **delete** use 0349's absolute-preserving paths, so hits
visibly stay put — the opposite of drag, and worth a different affordance
(double-click to insert, right-click to delete) so the two are not conflated.

Swing is one control per lane driving the warp. It is **self-documenting**:
uneven subdivision spacing is visible on the strip, so the user sees the swing
rather than reading a percentage. Show the number too, but the geometry is the
primary readout.

Sub-count is per lane with a per-beat override; the override is how tuplets are
entered, and needs a gesture on the beat rather than a separate mode.

## Acceptance criteria

- [ ] Dragging a beat marker moves hits in both adjacent slots, live, at
      interactive frame rates.
- [ ] Both adjacent slots are visually highlighted for the duration of the drag.
- [ ] The drag clamps at `MIN_SLOT` with visible feedback; no marker can be
      dragged past a neighbour.
- [ ] Insert and delete of a beat marker leave every hit visually stationary.
- [ ] Insert and delete have distinct affordances from drag.
- [ ] Outer markers are visibly non-draggable.
- [ ] The swing control redistributes subdivision markers within each beat, and
      hits at `f = 0` stay welded to their markers throughout the sweep.
- [ ] Per-beat sub-count override is settable from the strip; a beat set to 3
      shows three evenly-spaced subdivisions.
- [ ] Undo of a marker drag restores the marker and all apparent hit positions in
      one step.

## Notes

The welded-hit behaviour under a swing sweep is the demo that sells the model —
worth making it the thing to try first when this lands.

All redraw goes through `sub_pos` (0347). Translating hits by a pixel delta
during a drag will look right and be wrong: it double-applies once the model
updates, and drifts on a non-uniform grid.

## Close-out (2026-09-14)

**Engine.** Five `EngineCommand` variants carry the gestures
([io.rs:87](../../vxn-3/crates/vxn3-engine/src/io.rs#L87)): `DragBeatMarker`,
`InsertBeatMarker`, `DeleteBeatMarker`, `SetSwing` and `SetBeatSubs`. Three verbs
rather than one with a flag, because ADR 0007 §5's two rules are opposite and the
asymmetry belongs in the vocabulary — drag routes to 0349's `drag_beat_marker`
(relative, no hit record written), insert and delete to its absolute-preserving
pair, and swing / sub-count take `edit_grid`'s relative door so a welded hit rides
its marker. All five go through `apply_pattern_command`, so the main-thread model
and the audio thread's copy take them from one implementation. `EngineCommand` is
still `Copy`; `marker_gesture_drain_is_allocation_free` pins the insert/delete
re-derivation as allocation-free on the audio thread.

**Protocol.** Five opcodes in `parse_custom_ui`, with a new `f64_at`: a marker
position is an absolute beat position the whole geometry is measured against and
the `MIN_SLOT` argument is an `f64` one, so it is the one wire quantity that is
not narrowed to `f32`. Swing's shape and period read through `from_u8`, so an
unknown tag falls back rather than dropping an edit the user is mid-gesture on.
`build_html` now ships `min_slot`, since the strip draws the bound a drag is about
to clamp against and a page guessing at it would draw the wall in one place and
have the engine clamp in another.

**Faceplate.** A **grid rail** inside the top 14px of each strip
([app.js](../../vxn-3/crates/vxn3-ui-web/assets/app.js), `renderRail`) carries a
handle per beat marker and a cell per beat holding that beat's sub-count. Putting
the handles there rather than on the marker lines keeps a click on the strip a
placement whatever grid it lands on. `app.js` gained ports of
`Grid::set_beat_marker` / `insert_beat_marker` / `drop_marker` and of
`Pattern::edit_grid_preserving_times`, so the page applies the engine's own
clamped edit locally and the two lists stay index-for-index in step. Nothing
writes `markers[i]` outside `setBeatMarker`. `EDITOR_HEIGHT` follows the taller
row (420 → 530).

### Acceptance criteria

Verified by driving the real `build_html` page under a scratch DOM shim (vxn-3 has
no JS test infrastructure and none was added), plus Rust tests for the engine half:

- **Both adjacent slots move, live.** A drag of marker 1 moved the hit welded to it
  *and* the hit in the slot beyond it, per mousemove.
  `io::tests::marker_commands_keep_drag_relative_and_insert_absolute`,
  `marker_gestures_reshape_a_running_lane`.
- **Both highlighted for the duration.** Two `.slot-hi` elements during the drag
  (`stretch` and `squash`, coloured apart), zero after mouse-up.
- **Clamp visible from either side.** Two `.clamp-bound` lines at
  `m[i±1] ∓ MIN_SLOT` throughout, the one being pressed against marked `.at`, and
  the handle turning `.clamped`.
  `io::tests::a_marker_drag_command_clamps_instead_of_crossing` pins the engine's
  side, including the pinned outer markers.
- **Insert and delete move nothing.** Rendered hit `left`s were byte-identical
  across both, and `fire_beat` equal across both in the engine test. The insert
  index is `locate(pos).beat + 1` — the index that splits the slot the pointer is
  in; any other meets the clamp and reshapes a slot the user did not point at.
- **Distinct affordances.** Drag on a handle; double-click the rail background to
  insert; right-click a handle to delete.
- **Outer markers non-draggable.** `.mhandle.pinned` is hollow with
  `cursor: not-allowed`, refuses `mousedown` and says why in the status line.
- **Swing sweep keeps welded hits welded.** Markers redraw unevenly (0.0625 →
  0.08125 at +60% on a pair period) and the welded hit's rendered position stayed
  on a marker at every amount swept.
  `io::tests::a_swing_sweep_keeps_welded_hits_on_their_markers` asserts
  `fire_beat(i) == sub_pos(beat, sub)` exactly across the sweep.
- **Per-beat sub-count from the strip.** Drag the beat's number vertically (a
  click hands the beat back to the lane default). A beat set to 3 drew three
  markers at even thirds; exactness is pinned in Rust
  (`sub_pos(1, 1) == 1 + 1/3`).
- **Undo in one step.** ⌘/Ctrl+Z. With five hits spread across the two slots either
  side, a drag then an undo restored every marker *and* all five rendered hit
  positions exactly — one stored `f64` restores them all, because a drag writes no
  hit record.

### Review, and eight defects fixed before landing

An independent differential check re-ran the JS port and the Rust engine over the
same randomised op script (40 seeds × ~810 ops, positions parked against the
`MIN_SLOT` walls), comparing every marker, sub-count, swing field and each hit's
`(beat, sub, f, nudge, fire_beat)` as raw `f64` bits: zero divergence. The same
review found eight real defects in the gesture layer, all fixed and all now
covered by the regression harness:

1. `mousedown` fires for the right button too, so a right-click armed a drag that
   the delete beside it then pointed at the wrong marker. Left button only.
2. The same path let a right-click on a beat's number clear its override.
3. Double-clicking that number (which its own tooltip invites) also split the beat.
4. A rail insert left the `Bts` box stale, so the next spinner click sent
   `set_grid_beats` from the old number and re-laid every hand-placed marker.
   `refreshGeometry` now follows every geometry edit.
5. A beat-count or sub-count change left undo records naming markers that no
   longer exist. `dropUndo` now runs on any relayout, as it already did on a
   lane readback.
6. Delete-then-undo lost the *right* beat's sub-count override (a delete discards
   it, an insert inherits the left's), so an undone delete silently ate a triplet.
   The undo restores it — see the corrections below, which is how, and why the
   first attempt at it was worse than the bug.
7. The swing slider's pre-gesture snapshot is closure state a lane readback could
   not reach; `makeSwing` now exposes `cancel()`.
8. ⌘Z was captured with a text field focused, so fixing a typo in a voice name
   undid somebody's marker drag.

Also: a marker drag now re-asserts its final position on mouse-up, so a command
dropped by a full `EditQueue` mid-drag cannot leave the engine on an older marker
with nothing following to correct it.

### Three corrections after that round

A second review pass over the fixes above found that two of them were incomplete,
and a third defect in the correction itself.
Both are fixed here.

**Fix 6 traded a lost override for moved hits.** Restoring the override as a
*separate* `set_beat_subs` after the insert could not work, and the reason is
structural rather than incidental: an insert takes the **absolute** door (hits
hold their time, `(beat, sub, f)` is rebuilt under them) while
[`SetBeatSubs`](../../vxn-3/crates/vxn3-engine/src/io.rs) takes the **relative**
one by design (hits hold `(beat, sub, f)` and the grid moves under them). In
sequence the pair composes to *"preserve times, then move everything in that
beat"*. Measured, a hit in the restored beat moved 0.111 beats — ~55 ms at
120 bpm — while the status line said *"the hits either side did not move."* The
override was back and the times were gone; before the fix it had been the other
way round.

The override now **rides the insert**: `EngineCommand::InsertBeatMarker` carries
`subs`, and `Pattern::insert_beat_marker_with_subs` applies both inside one
`edit_grid_preserving_times`, so the combined change is what times are preserved
across. A refused insert applies no sub-count, or a no-op edit would silently
retime a beat. Pinned by
`sequencer::tests::undoing_a_delete_restores_the_override_without_moving_a_hit`
and `a_refused_insert_with_subs_changes_nothing`. The page's port mirrors it, so
the two stay index-for-index in step.

`subs` **states** the new beat's count rather than defaulting, and that is the
third correction. `Grid::insert_beat_marker` gives the new beat the *split beat's*
override, so on a tuplet "leave it alone" and "clear it" are different grids — and
the first version of this had `subs: 0` clear the override in Rust while the page's
mirror kept the inherited one, which is a page/model divergence over an ordinary
insert, the exact failure the port exists to prevent. An ordinary insert now sends
what the split inherits and only undo-of-delete sends anything else. Pinned by
`sequencer::tests::a_stated_sub_count_is_not_the_same_as_a_cleared_one`, and
checked on the page path: splitting a triplet sends `subs: 3` and yields two
triplets on both sides.

That test asserts a **tolerance**, not `f64` equality, unlike the single-edit
insert/delete tests — and the size of it is the claim. `Hit::f` is `f32`, so a
*round trip* (re-derived by the delete, re-derived again by the insert) quantises:
a hit welded at `f = 0` lands mid-slot in the merged slot, and that fraction is not
exactly representable. The residue is ~1e-8 beats. The bound is 1e-6, five orders
of magnitude below the defect it pins, so the bug fails it loudly.

**The mouse-up re-assert was gated on the undo record's question.** Both the
re-assert and the `pushUndo` sat behind `moved && markers[i] !== from`. A marker
parked at its `MIN_SLOT` wall, dragged away and back, ends exactly where it
started — so `moved` is true, `markers[i] === from`, and the re-assert was skipped
on precisely the longest command stream, i.e. the drag most likely to have had one
dropped. The re-assert is now gated on `moved` alone; `!== from` still guards the
undo record, since restoring a position the marker already holds is not an edit.

### Not verified

- **No browser was driven.** vxn-3 has no wasm build or browser harness. Markup,
  CSS and the opcode round trip were checked structurally and the gestures were
  exercised under a DOM shim, but nobody has looked at the rendered page. The
  colours, the 14px rail height and the legibility of the sub-count badge are
  unverified visually.
- "Interactive frame rates" is argued, not measured: a mousemove rebuilds one
  lane's rail, markers and ≤64 diamonds, and sends at most one command.
- `cargo fmt` was not run (the tree is not rustfmt-clean); new code is
  hand-formatted to match its surroundings. `cargo clippy` is clean for the vxn-3
  crates.
- Sub-count *decrease* re-snaps the hits whose slot it removed, which is the
  relative rule and correct, but it is the one geometry gesture that can stack two
  hits on one marker. Left as is.
