---
id: "0058"
title: "Drop bespoke pushes + echoes: state-load, SetMatrixRow echo, main.js mtxN-depth hack"
priority: high
created: 2026-06-10
epic: E005
depends: ["0055", "0056", "0057"]
---

## Summary

Fourth ticket of [E005](../../epics/open/E005-dirty-bitset-pump.md).
With the bitset pump covering all Model drift (matrix included), every
bespoke "I just changed the Model, also push a view event" call site
becomes redundant. Delete them. The pump is the single bridge.

Per [ADR 0003](../../adrs/0003-dirty-bitset-diff-pump.md) §"Removed":
state-load push, `SetMatrixRow` echo, and the `mtxN-depth` regex sync
hack in `main.js`. None of them survive after the bitset covers the
ground.

## Acceptance criteria

- [ ] `PluginStateImpl::load` in `vxn2-clap/src/lib.rs` reverted to
  its pre-hotfix shape: read blob, call
  `ParamModel::load_bytes(&*self.shared.params, &blob)`, return.
  No `push_matrix_snapshot` call. (State load now flips dirty bits
  inside `load_bytes` — verified in 0055; the pump catches them on
  next tick.)
- [ ] `Vxn2UiCustom::SetMatrixRow` handler in
  `vxn2-app/src/controller.rs:47-57` no longer pushes a
  `MatrixRowChanged` view event after `ctrl.model().set_matrix_row`.
  Body becomes one line: `ctrl.model().set_matrix_row(slot, row);`.
  (Pump catches the drift on next tick. Optimistic UI paint —
  `dispatchRow` writing `vxn.matrix.rows[slot]` before dispatching —
  covers the one-tick latency.)
- [ ] `Vxn2UiCustom::SetOpTab` handler in
  `vxn2-app/src/controller.rs:42-46` **keeps** its `OpTabChanged`
  echo. That's pure UI mode state with no Model backing — the bitset
  doesn't carry it. Document the asymmetry.
- [ ] `vxn2-ui-web/assets/main.js` — the `mtxN-depth` regex sync
  block inside the `param_changed` handler is removed. The standard
  primitive bind already updates the depth slider on
  `param_changed` (depth lives in `values`); the matrix overlay's
  full-row state arrives via the per-tick `MatrixSnapshot` push
  from the pump.
- [ ] Manual test (Reaper / Bitwig):
  - Open host with a saved patch that has populated mod-matrix
    routes. Open the mod-matrix overlay. Verify rows render with the
    saved topology immediately, no overlay-close-reopen needed.
  - Edit a topology field (source, dest, curve, active) from the
    UI. Verify it persists across a Reaper save/load round-trip and
    re-renders correctly on cold open.
  - Edit slot 1-8 depth from the UI. Verify the matrix slider and
    any other bound widget for that param both update together. No
    fight, no flicker.
  - Bind host automation to `mtx1-depth`. Verify the matrix overlay
    slider follows the automation (without the deleted `main.js`
    sync hack).
- [ ] `cargo build --workspace` green.
- [ ] `cargo test --workspace` green.
- [ ] No regression in existing matrix tests in `vxn2-app` or
  `vxn2-engine`.

## Notes

This is the "delete code" ticket. Everything it removes was added as
a workaround for the missing Model → View bridge over non-CLAP
fields. The bitset covers all of it.

Order matters: don't ship this without 0055 + 0056 + 0057. Removing
the bespoke pushes before the pump covers matrix reintroduces the
original bug (matrix drift invisible to the view).

If 0057 left the gesture gate in the pump (as suggested in its
notes), the host-automation-vs-mid-drag invariant still holds: pump
skips ids whose gesture bit is set. 0060 moves the gate to the view
later; until then, the pump's gate is what protects against
mid-drag flicker.
