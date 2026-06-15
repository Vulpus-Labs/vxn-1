---
id: "0050"
product: vxn-3
title: "vxn-3 p-locks — step shape (revert + latch)"
priority: medium
created: 2026-06-15
epic: E021
depends: ["0048"]
---

## Summary

Per-step parameter locks — the step-shape subset of ADR 0001 §3a (revert +
latch, no ramp). Drives per-hit timbre variation and, with the send param in
0051, the dub throw. Ramp/curve behaviours are deferred.

## Design

- **Lock record (subset).** `{ param, value, shape: step, termination: revert |
  latch }` plus `N` for revert. No `ramp` shape, no curve in this slice.
- **Behaviours.**
  - *revert*: jump to value, hold `N` lane-local ticks, then release. `N = 1`
    is the momentary spike.
  - *latch*: jump to value, hold until the next lock on that param; persists
    across the loop boundary (loop 1 differs cold — intended).
- **Resolution per tick.** Layered: `effective = base`, then the active lock
  override on top. *revert* releases → fall back to base. New lock on a param
  immediately supersedes any in-flight hold (preemption, no queue).
- **Scope.** Lockable params = continuous track/engine params (and the send
  amount once 0051 exists). Trig attributes (probability, retrig, velocity) are
  *not* p-locks — they live on the trig (0048).
- **`N` units.** Lane-local ticks (same base as 0048), so a hold tracks the
  lane's grid.
- **Storage.** Per track, a sparse `(param, step) → lock` table; resolver keeps
  a small per-locked-param state struct (value, ticks-left, termination).
  Preallocated (bounded by engine param count), allocation-free in `process`.

## Acceptance criteria

- [ ] A revert lock with `N = 1` changes a param for one tick then returns to
      base; `N > 1` holds then releases.
- [ ] A latch lock holds its value until the next lock on that param and
      persists across loop wrap.
- [ ] A new lock mid-hold immediately supersedes the prior one.
- [ ] Resolution is layered base→override and matches §3a for the step subset.
- [ ] Lock table + resolver are allocation-free on the audio thread.

## Notes

- Ramp/curve (`fast-start` / `slow-start` / `S`), and treating the per-param
  lock timeline as an "automation lane" view, are deferred post-MVP.
- Design: `vxn-3/adrs/0001` §3a.

## Close-out (2026-06-15)

- **Lock record (step subset).**
  [sequencer.rs](../../vxn-3/crates/vxn3-engine/src/sequencer.rs): `LockParam`
  (Gain/Pan/Decay/Tone/Pitch — continuous params; trig attributes deliberately
  excluded), `Termination::{Revert{n}, Latch}`, `Lock { value, termination }`.
  `Pattern` carries a sparse `(step, param) → Option<Lock>` table
  (`set_lock`/`clear_lock`/`lock_at`). No ramp/curve (deferred).
- **Behaviours + resolution.** The per-track resolver lives in `LaneState`
  ([lane.rs](../../vxn-3/crates/vxn3-engine/src/lane.rs)): `override_val` +
  `revert_ticks`, advanced by `process_locks` at every crossed boundary in the
  same walk as trigs. Reverts tick down first, then the step's locks apply.
  Effective resolution is layered `override ?? base` in
  [track.rs](../../vxn-3/crates/vxn3-engine/src/track.rs) `apply_effective`
  (base[] = UI-set, applied[] memoised so knobs re-cook only on change). Tests
  `lane::tests::revert_n1_holds_one_tick` (N=1 = one tick then base),
  `revert_n2_holds_then_releases` (N>1 holds then releases),
  layering via `plocks::gain_latch_silences_the_mix`.
- **Latch + loop wrap.** `lane::tests::latch_holds_until_next_lock_and_across_wrap`
  — a latch persists past the pattern wrap until superseded.
- **Preemption.** `lane::tests::new_lock_preempts_in_flight_hold` — a new lock
  mid-hold immediately replaces the prior revert (no queue).
  `transport_jump_clears_holds` covers the seek case.
- **Lane-local `N`.** Ticks decrement per crossed lane boundary, so a hold
  tracks the lane's own grid (polymeter-consistent with 0048).
- **Allocation-free.** Sparse table is in the (pre-allocated, `Copy`) `Pattern`;
  resolver is fixed `[_; N_LOCK_PARAMS]` arrays. Test
  `plocks::lock_resolution_is_allocation_free` — 0 allocs over ~300 blocks while
  pushing live `SetLock` edits.
- *NB:* resolution is **block-rate** (matches vxn-3's block-rate param
  granularity); sample-accurate p-locks are post-MVP. Lockable **send amount**
  joins with 0051; faceplate lock-editing UI is a 0052 follow-up (the engine +
  `EngineCommand::SetLock`/`ClearLock` path is ready).
- 58 vxn3 tests green; vxn3 crates clippy-clean; `clap-validator` 0 failures.
