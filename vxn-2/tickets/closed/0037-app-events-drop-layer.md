---
id: "0037"
title: App + events — drop `Layer` enum and edit-layer events
priority: medium
created: 2026-06-09
epic: E004
---

## Summary

Fifth ticket of [E004](../../epics/open/E004-single-layer-collapse.md).
Remove the `vxn2_app::model::Layer` enum and every event variant that
threads a layer through the controller. The Upper/Lower edit toggle
disappears from the model layer; **0038** handles the corresponding
JS/HTML changes.

Per [ADR 0002](../../adrs/0002-drop-dual-layer.md).

## Acceptance criteria

- [ ] `crates/vxn2-app/src/model.rs`:
  - [ ] `Layer` enum (lines 12-31) deleted.
  - [ ] `matrix_row(layer: Layer)` / `set_matrix_row(layer: Layer)` /
        `edit_layer()` / `set_edit_layer()` trait methods deleted.
  - [ ] The trait now exposes `matrix_row()` / `set_matrix_row()`
        without a layer arg.
- [ ] `crates/vxn2-app/src/events.rs`:
  - [ ] `use crate::model::Layer` removed.
  - [ ] `SetEditLayer` event deleted.
  - [ ] `SetOpTab { layer }` → `SetOpTab` (no `layer` field).
  - [ ] `OpTabChanged { layer }` → `OpTabChanged` (no `layer` field).
  - [ ] `EditLayerChanged` event deleted.
  - [ ] `MatrixSnapshot { upper, lower }` → `MatrixSnapshot { rows }`
        (or equivalent single-table shape).
- [ ] `crates/vxn2-app/src/controller.rs`:
  - [ ] `Layer` imports + `snapshot_layer()` helper removed.
  - [ ] `SetEditLayer` handler removed.
  - [ ] Matrix snapshot built from the single table.
- [ ] `crates/vxn2-app/tests/controller.rs`:
  - [ ] All `Layer::Upper` / `Layer::Lower` references removed.
  - [ ] Tests assert single-table matrix snapshots.
- [ ] `cargo build -p vxn2-app` green.
- [ ] `cargo test -p vxn2-app` green.

## Notes

Sequenced after 0033 (engine flat) but parallel-safe with
**0034 / 0035 / 0036**. The app crate consumes the engine's
`Patch` and `SharedParams` shape; once those are flat, this ticket can
proceed independently of the CLAP refit.

The `edit_layer` view state was already not a CLAP param (per ticket
0009 closing notes — "non-automatable view state"). Removing it from the
event surface costs no migration; there is no host state holding it.

If any test mocks `Layer::Upper` to verify routing, drop the test
entirely — the behaviour it was verifying (correct dispatch to the
upper table) no longer exists as a concept.

The downstream JS bridge (`vxn2-ui-web/src/lib.rs`) listens to these
events. Its rewrite is **0038**'s scope. This ticket must coordinate
with 0038 so that the JS bridge isn't simultaneously consuming an
`EditLayerChanged` event that no longer fires.

The `SetOpTab` / `OpTabChanged` rename retains the tab index (`op:
u8`); only the `layer` field disappears.
