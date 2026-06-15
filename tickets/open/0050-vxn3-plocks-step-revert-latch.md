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
