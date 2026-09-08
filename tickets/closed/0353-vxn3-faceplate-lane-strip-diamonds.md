---
id: "0353"
product: vxn-3
title: "Faceplate: continuous lane strip with draggable diamond hits, snap and quantise"
priority: medium
created: 2026-09-04
epic: E050
depends: ["0348"]
---

## Summary

The first faceplate ticket of
[E050](../../epics/open/E050-vxn3-continuous-lane-editor.md), implementing
[ADR 0007](../../vxn-3/adrs/0007-vxn3-continuous-lane-editor.md) §1. Replaces the
step-grid lane in
[app.js](../../vxn-3/crates/vxn3-ui-web/assets/app.js) with a continuous
rectangular strip per track: **X is time, Y is a modulation value**, and hits are
draggable diamonds placed freely in it.

## Design

The strip draws the marker geometry from 0347 — beat markers heavy, subdivision
markers thin and dim — and hits as diamonds at their resolved positions. The grid
is **drawn, not stored into**; a diamond's position is 0348's `(beat, sub, f,
nudge)` plus `y`, resolved for display.

**Snap and quantise are editor verbs, not storage constraints** (ADR 0007 §1). A
quantised hit is one whose stored `f` happens to be zero; nothing in the data
model knows the difference.

- **Snap** is a toggle applied during drag — the diamond lands on the nearest
  subdivision marker with `f = 0`.
- **Quantise** is applied after the fact to already-placed hits, and gets two
  independent verbs: quantise **X** to the nearest subdivision marker, and
  quantise **Y** to the groove centre curve. Partial quantise lerps `f` toward 0
  or 1 (whichever is nearer) and decays `nudge` by the same amount.

Snap targets are exactly the subdivision markers, beat markers included (0347).

## Acceptance criteria

- [ ] Each track renders as a continuous strip with beat markers visually
      distinct from subdivision markers.
- [ ] Hits render as diamonds and drag freely in X and Y within the strip.
- [ ] Snap toggles; with snap on, a dropped diamond stores `f = 0, nudge = 0` and
      sits exactly on a subdivision marker.
- [ ] Quantise-X and quantise-Y are separate commands and can be applied
      independently to a selection.
- [ ] Partial quantise lerps `f` toward the nearest marker and decays `nudge`
      proportionally.
- [ ] Dragging a hit across a beat marker updates its `(beat, sub)` and
      recomputes `f`; the hit does not visually jump in X.
- [ ] Playhead renders against the strip and tracks the swung grid.
- [ ] Hit add and delete work in the strip; the `MAX_HITS` ceiling is enforced in
      the editor with visible feedback rather than a silent drop.

## Notes

Depends only on 0348. Marker dragging is 0354, the palette is 0355, curve editing
is 0356 — this ticket ships the strip with a static grid, which is enough to
place and hear freely-positioned hits.

Y renders relative to the groove's centre curve once 0350/0356 land; until then a
flat curve makes that reduce to absolute-in-lane, so no rework is needed to
sequence it this way.

Watch the swung grid: subdivision markers are unevenly spaced by construction, so
any pixel-per-step assumption inherited from the old step grid is a bug. Position
comes from `sub_pos`, never from multiplication.

## Close-out (2026-09-07)

Shipped in three commits.

**Engine.** An appended `impl Pattern` block carries the editor's hit-keyed
vocabulary: `set_position` (the drag), `quantise_x` / `quantise_y` as two
independent verbs, `y_centre` (flat at `Y_CENTRE` until 0350 stores control
points), and `set_hit_y` / `set_hit_note` / `set_hit_probability` /
`set_hit_retrig`. Position edits go through `remove` + `insert` so fire order and
the geometry clamp are re-established by the code that already owns them, and the
hit's new index comes back to the caller. Nine matching `EngineCommand` variants
cross the edit queue; `EngineCommand` is still `Copy` and the drain is still
allocation-free (`command_drain_is_allocation_free` now pushes `SetHitPosition`).
All three `apply_command` match arms were updated — the extraction arm has no
catch-all, so a missed variant is a compile error rather than a silent no-op.

**Faceplate.** `build_html` ships real **per-lane** geometry (beat markers,
per-beat sub counts and overrides, the swing warp as its tag encoding) plus the
engine's limits, and `app.js` ports `grid.rs` to place markers from it. Every X
comes from `subPos`; there is no pixels-per-step multiplication and no flex row
of equal cells anywhere in the strip or its CSS.

**Rule changed during review.** `quantise_x` previously chose its target from `f`
alone, so a hit in the lane's **final** slot with `f > 0.5` lerped toward the
pattern end — a marker the full quantise will never land on. Sweeping the amount
control dragged such a hit later and later, then snapped it a whole slot earlier
at the top. `next_marker` now returns `Option` and the final slot has no forward
target at any amount. Pinned by
`the_last_slot_quantises_backwards_not_off_the_end`, which now asserts
monotonicity across the whole amount range.

### Acceptance criteria

All eight verified, six of them by running `app.js` headless against the real
`build_html` config under a scratch DOM shim (the repo has no JS test
infrastructure for vxn-3 and none was added):

- Strip per track with 4 beat + 1 end + 12 sub markers, distinct classes,
  strictly increasing positions.
- Diamonds render and drag in X and Y; a 201-step sweep across two beat markers
  showed 0 jumps and 0 backslides, with 3 marker crossings following in
  `(beat, sub)`.
- Snap on → `add_hit` with `f = 0, nudge = 0` landing exactly on a marker;
  toggling it off keeps the in-slot fraction.
- Quantise-X and quantise-Y send different opcodes and neither touches the other.
- Partial quantise lerps `f` and decays `nudge` (Rust unit test, plus the JS
  mirror checked value-for-value against the engine).
- Playhead renders against the strip and, on a fully swung lane, sits at 0.375
  beats for slot 1 rather than 0.25 — it tracks the warped grid.
- Add and delete work; the lane fills to 64 and stops, flashes the strip, says so
  in the toolbar, and sends no over-capacity `add_hit`.

The port's exactness was checked differentially against the shipped Rust: 14,579
values across 12 grid configurations (straight, full/negative MPC, beat-wide and
`Custom(5)` periods, triplets, a tuplet override, skewed markers, 16 subs, a
single-slot lane, four beat-count relayouts) — `sub_pos`, `locate`, `fire_beat`,
insertion order, `relayoutBeats` markers and `quantise_x`'s returned index and
stored position — with zero mismatches. The adversarial review ran its own
independent harness over 202k values and also found none.

### Not verified, and one gap to carry

- **No browser was driven.** vxn-3 has no wasm build, dev server or browser
  harness; the static `preview` dump is the only browser-visible artifact. It was
  grepped (strip / marker / diamond / playhead / rackbar CSS present, no `div.cell`
  or `.steps` markup left) and the behaviour was exercised under a DOM shim, but
  the rendered page has not been looked at by a human.
- **The page's hit list is seeded empty and never read back.** Every hit-keyed
  edit names a fire-order index, and `serialise_custom_view` carries only the
  playhead. That is correct for a fresh instance and wrong for an editor reopened
  over a lane that already holds hits (GUI close/reopen, or a `clap.state`
  restore): the two lists then disagree and later index-keyed edits address the
  wrong hit. **This wants its own ticket** — a hit-list readback on the view
  channel. Clearing the engine's lanes on load would buy agreement by throwing
  away a restored pattern, so it was not done. Noted in both `app.js` and
  `vxn3-ui-web/src/lib.rs` at the code that depends on it.
- `cargo fmt` was **not** run: the tree is not rustfmt-clean (a bare
  `cargo fmt -p vxn3-engine` rewrites ~1,400 lines across files this ticket does
  not own), so the new code is hand-formatted to match its surroundings.
  `cargo clippy` is clean for the vxn-3 crates.
- Per-beat sub-count overrides are plumbed end to end (shipped in the geometry
  JSON, honoured by the port) but no gesture in this ticket creates one, and no
  gesture produces a non-zero `nudge` — `positionFor` always stores the offset as
  `f`. Both paths are exercised by the differential check, not by the UI.
