---
id: "0346"
product: vxn-3
title: "Continuous-timeline lookahead scheduler for the lane, behaviour-preserving"
priority: high
created: 2026-09-04
epic: E050
depends: []
---

## Summary

First ticket of [E050](../../epics/open/E050-vxn3-continuous-lane-editor.md).
Replace the step-boundary walk in
[`LaneState::schedule`](../../vxn-3/crates/vxn3-engine/src/lane.rs#L174-L204)
with the continuous-timeline lookahead loop
[ADR 0004](../../vxn-3/adrs/0004-vxn3-micro-timing.md) §3 specified and
[ADR 0007](../../vxn-3/adrs/0007-vxn3-continuous-lane-editor.md) §9 requires.

Today the lane computes `let first = (beat0 / sb).ceil()`, walks whole-step
boundaries falling inside the block, and fires at each. No hit can be scheduled
off its boundary — the one exception,
[`emit_retrig`](../../vxn-3/crates/vxn3-engine/src/lane.rs#L211), carries a single
in-flight window across blocks by hand. ADR 0004 called this out in advance:
*"the pattern-engine scheduler is a continuous-timeline lookahead loop from the
start … retrofitting lookahead later is a rewrite."*

This ticket lands the rewrite **alone and behaviour-preserving**, on today's
`[Step; 16]` grid, so it can be verified against the existing tests before the
data model changes under it (0348).

## Design

Fire times become points on a continuous per-lane beat timeline rather than
indices. Per block:

1. Advance a **bounded lookahead window** — const-sized, since offsets are
   bounded — over the timeline, resolving candidate fire times into it.
2. Emit everything in the window landing in `[beat0, beat_end)`, frame-ordered.
3. Carry the remainder to the next block.

Retrig stops being special-cased state: a retrig macro expands into `n` fire
times in the window at resolve time, which subsumes the seven `rt_*` fields on
[`LaneState`](../../vxn-3/crates/vxn3-engine/src/lane.rs#L24-L50). Probability is
still drawn **once per primary trig** — the draw moves to window-resolve time,
and must stay un-re-rolled when a trig straddles a block boundary, which is the
property `last_index` protects today.

The p-lock resolver
([`process_locks`](../../vxn-3/crates/vxn3-engine/src/lane.rs#L94)) still runs
per crossed grid position, independent of trigs, in position order. Transport-jump
resync drops the window along with the in-flight state it replaces.

Storage is fixed-capacity: the window is preallocated and an over-capacity push
drops a hit rather than allocating, matching
[`push_hit`](../../vxn-3/crates/vxn3-engine/src/lane.rs#L251).

## Acceptance criteria

- [ ] `LaneState::schedule` resolves fire times on a continuous timeline with a
      bounded lookahead window; no step-boundary walk remains.
- [ ] The `rt_*` in-flight retrig fields are gone; retrig expands into window
      entries at resolve time.
- [ ] Every existing test in
      [lane.rs](../../vxn-3/crates/vxn3-engine/src/lane.rs#L261) and
      [tests/pattern.rs](../../vxn-3/crates/vxn3-engine/tests/pattern.rs) passes
      unchanged — no test edits to accommodate the rewrite.
- [ ] `tests/groove.rs`'s allocation trap stays armed and reports zero
      allocations in `process`.
- [ ] Probability is drawn once per primary trig across a block boundary —
      existing coverage retained, plus a test that splits one trig's step across
      two blocks at several split points and asserts one draw.
- [ ] Transport-jump resync clears the window; the existing
      `transport_jump_clears_holds` behaviour is unchanged.
- [ ] Render output is bit-identical to the pre-ticket build for the E021 demo
      pattern (capture a reference render before starting).

## Notes

Behaviour-preserving is the whole point — this ticket adds no offsets, no
markers and no new fields. Offsets arrive in 0348 once the timeline exists to
carry them.

The window bound comes from the monotonic-fire-order invariant (ADR 0007 §9).
On today's grid every offset is zero so the window is trivially one position
deep; size it from the invariant now rather than hard-coding zero, or 0348 has
to revisit this file.

Out of scope: the hit list, markers, Y, colour. All of E050's later tickets
depend on this one but none of them are started by it.

## Close-out (2026-09-09)

Landed 2026-09-04 as `ea1b4c6`. Closed retrospectively — the code shipped and the
epic has since built four more tickets on top of it.

- `LaneState::schedule` resolves fire times on a continuous per-lane beat
  timeline. The step-boundary walk is gone: no `(beat0 / sb).ceil()` remains in
  [lane.rs](../../vxn-3/crates/vxn3-engine/src/lane.rs), and the block loop
  advances a bounded window rather than whole-step boundaries.
- The window is a const-sized preallocated `[Pending; WINDOW_CAPACITY]` with
  `Window::{push, clear, drop_retrig_tail, emit_due}`; an over-capacity push
  drops rather than allocating.
- `LOOKAHEAD_POSITIONS` is derived from the offset bounds rather than hard-coded,
  behind `const _: () = assert!(LOOKAHEAD_POSITIONS >= MAX_EARLY_SLOTS +
  MAX_LATE_SLOTS + 1.0)` — which is what let 0348 raise the real bound by editing
  one constant instead of revisiting the file, exactly as this ticket's Notes
  asked.
- The seven `rt_*` in-flight retrig fields are gone; `expand_retrig` writes `n`
  fire times into the window at resolve time.
- Probability is drawn once per primary trig across a block split —
  `probability_is_drawn_once_per_trig_across_block_splits`.
- Transport-jump resync drops the window and its in-flight state —
  `transport_jump_drops_the_lookahead_window`, alongside the unchanged
  `transport_jump_clears_holds`.
- The allocation trap in `tests/groove.rs` stayed armed and green.

**Two acceptance criteria were point-in-time and cannot be re-verified now.**
"Every existing test passes unchanged" and "render output is bit-identical to
the pre-ticket build for the E021 demo pattern" were both checked against the
`[Step; 16]` data model at the time of landing. 0348 replaced that model outright
and rewrote those tests, as it was scoped to do. The behaviour-preserving claim
stands as of `ea1b4c6`; it is not re-checkable against the current tree, and
nothing after 0348 depends on it being so.

Superseded in part by 0348, which set `EARLY_SLOTS` to its real value and made
`WINDOW_CAPACITY` a function of `MAX_HITS`, and which added cursor re-anchoring
so a live grid edit cannot strand the lane.
