---
id: "0247"
product: vxn-1b
title: "Engine→page mod-matrix topology echo (preset / state load under an open editor)"
priority: medium
created: 2026-08-07
epic: E038
depends: ["0246"]
---

## Summary

Follow-on to [0246](0246-vxn1b-matrix-topology-stale-on-gui-reopen.md), which
fixed the *open-time* seed. The remaining half: with the editor **already open**,
loading a preset or a host project — or a host undo — rewrites the shared store's
topology while the page keeps painting the routing it was seeded with. Depth
dials follow (CLAP params, echoed by `push_param_diffs`); source/dest/curve/scale
do not, because topology is not a param and nothing in `ViewEvent` carried it.

Same failure mode as 0246 and the same hazard: a combo showing a stale source
will overwrite the real route with that stale value the moment it is touched.

## Design

A per-tick diff beside the existing param pump, reusing the custom view payload
channel that already carries meter frames:

- `MatrixSnapshot { layers: [MatrixTable; 2] }` — a view payload in
  [matrix.rs](../../vxn-1b/crates/vxn1b-engine/src/matrix.rs#L306). Topology
  only; depths stay on `ParamChanged` so the page never has two sources of truth
  for one value.
- `VxnMainThread::push_matrix_echo` diffs `matrix_snapshot()` against
  `last_matrix` each tick and pushes on drift. `last_matrix` starts `None` and is
  reset to `None` when the GUI closes, so every open re-pushes once rather than
  trusting a snapshot the new page never received.
- `serialise_custom_view` gains a `MatrixSnapshot` arm emitting
  `{kind: "matrix", slots: [[…], […]]}`. Both it and the open-time seed go
  through one `slots_json` writer, so the two wire shapes cannot drift apart.
- `dispatch.js` swaps `window.vxn.matrix.slots` and calls
  `matrixOverlay.refreshForLayer`. Reflect-only — no `set_matrix` is posted, so
  an echo cannot bounce the load's own routing back at the engine.

Page edits round-trip through the diff too. That is deliberate: the combo
already shows what it posted, so reflecting is a no-op, and it keeps this a
plain "the store is the truth" diff rather than an origin-tracking scheme.

## Acceptance criteria

- [x] Topology change under an open editor pushes exactly one echo; no echo
      while nothing changes (diffed, not broadcast every tick).
- [x] `dispatch.js` handler swaps the snapshot + repaints, posts no `set_matrix`,
      and ignores a malformed echo rather than blanking the snapshot
      (`dispatch-orchestration.test.js`).
- [x] Echo and seed agree on the slot wire shape, depth excluded
      (`echo_slot_shape_matches_the_open_time_seed`).
- [ ] **In a DAW:** with the editor open, load a preset — the combos follow.
      Then host-undo and check they follow back.

## Notes

- Cost is a 32-slot struct compare per 60 Hz tick, next to the param diff loop
  already running there; the push itself is change-gated.
- The echo rides the existing per-tick batch — one `evaluate_script`, no new
  bridge channel.
- **Id collision:** a concurrent session is holding an untracked
  `tickets/open/0246-vxn1b-oversampling-and-limiter.md`; the committed 0246 is
  the matrix-topology fix this depends on. One of them needs renumbering.
