---
id: "0022"
title: vxn2-app crate scaffold (Controller, ParamDesc, UiEvent / ViewEvent)
priority: high
created: 2026-06-06
closed: 2026-06-06
epic: E003
---

## Summary

Stand up the `vxn2-app` crate as the controller-surface composition layer
for VXN2: bridge `vxn2-engine::SharedParams` to `vxn_core_app::Controller`,
declare VXN2-specific `UiEvent::Custom` / `ViewEvent::Custom` payloads
(op-tab switch, mod-matrix row edits, edit-layer toggle), and supply the
closure helpers `vxn2-clap` will pass to `Controller::tick`.

The `impl vxn_core_app::ParamModel for SharedParams` lives in `vxn2-engine`
itself — orphan rule — same shape as `vxn-1/crates/vxn-engine/src/shared.rs`
post-core-extraction. This ticket also adds the gesture flag array
`SharedParams` lacks today.

`vxn2-app` is thin: it carries the bits the audio kernel doesn't own
(custom event types, the `tick` helper that wires the closure pair,
re-exports of `desc_for_clap_id` / `module_for_clap_id` / `TOTAL_PARAMS`)
without re-implementing the controller event loop.

## Acceptance criteria

- [ ] `vxn-2/crates/vxn2-app` added to the workspace `members` list
      and given a `workspace.dependencies` alias `vxn2-app = { path = "vxn-2/crates/vxn2-app" }`.
- [ ] `vxn2-engine` depends on `vxn-core-app` (workspace) and gains:
      - A `[u8; TOTAL_PARAMS]` (or compact bitset) gesture array on
        `SharedParams` with atomic load/store helpers.
      - `impl vxn_core_app::ParamModel for SharedParams` — pure
        delegation to the existing inherent methods. `get_normalized`
        forwards to the British-spelled `get_normalised` (no rename to
        avoid touching the engine call sites).
      - `ParamModel::descriptor(id)` returns
        `&'static vxn_core_app::ParamDesc` from a const conversion
        table built next to the engine's `PARAMS` array: each
        `vxn2_engine::ParamDesc { id, name, ... }` maps to
        `vxn_core_app::ParamDesc { name: id, label: name, ... }`. No
        runtime alloc; one `const` array.
      - `snapshot_bytes` / `restore_from_bytes` round-trip the
        existing `SharedParams ↔ EngineParams` blob (magic / version
        from 0017).
- [ ] `vxn2-app` crate layout mirrors `vxn-1/crates/vxn-app`:
      - `lib.rs` — re-exports + crate docs.
      - `events.rs` — `Vxn2UiCustom` / `Vxn2ViewCustom` enums:
        `SetEditLayer { layer: Layer }`, `SetOpTab { op: u8 }`,
        `SetMatrixRow { slot: u8, source: MatrixSource, dest: MatrixDest, curve: MatrixCurve, active: bool }`,
        `MatrixRowChanged { slot: u8, ... }` (view echo).
      - `controller.rs` — a `tick_vxn2(controller, params)` helper that
        builds the `(on_custom_ui, on_custom_host)` closure pair and
        calls `Controller::tick`. Handles `SetMatrixRow` by writing
        through to `Vxn2Params::set_matrix_row` then emitting
        `MatrixRowChanged` for the view echo.
      - `model.rs` — `Vxn2Params: ParamModel` extension trait for
        non-CLAP shared state: matrix rows (slot index, source / dest /
        curve / active flags), edit-layer view state.
- [ ] `vxn2-engine::SharedParams` implements `Vxn2Params` with
      atomic per-slot matrix-row storage (16 slots × 2 layers, each row
      ≤ 8 bytes).
- [ ] Unit test in `vxn2-app`: construct a controller against
      `SharedParams`, post `UiEvent::SetParam { id, plain }`, tick
      with `tick_vxn2`, assert a `ViewEvent::ParamChanged` arrives on
      the view-rx with the matching plain / norm / display.
- [ ] Second unit test: post
      `UiEvent::Custom(Vxn2UiCustom::SetMatrixRow { ... })`, tick,
      assert `Vxn2Params::matrix_row` returns the updated row AND a
      `ViewEvent::Custom(Vxn2ViewCustom::MatrixRowChanged)` lands on
      the view rx.
- [ ] `cargo build -p vxn2-app` and `cargo build -p vxn2-engine`
      both succeed; `cargo test -p vxn2-app` passes.

## Notes

- The `Layer` enum is the per-voicing-mode index (Upper = 0, Lower = 1).
  VXN1 owns it in `vxn-app/src/domain.rs`; VXN2 defines its own
  identical enum in `vxn2-app/src/model.rs` (the two synths don't share
  domain enums — different patches, different state blobs).
- `KeyMode` is VXN1-specific (Whole / Dual / Split). VXN2 has
  `VoicingMode` (Whole / Layer / Split) which is a CLAP-automatable
  enum param (id `voicing_mode` in PARAMETERS.md), so it does NOT need
  a `Vxn2Params` extension. `edit_layer` IS view state and lives on
  the extension trait.
- Mod-matrix `source` / `dest` / `curve` are not CLAP params (topology
  selectors); `depth` slots 1-8 ARE CLAP params per layer (16 total
  CLAP slots — handled via plain `UiEvent::SetParam`). The `Custom`
  path only fires for the topology bits + active flag + depths 9-16.
- Module strings for the host's auto-grouping (`module_for_clap_id`)
  already exist in `vxn2-engine`; `vxn2-app` re-exports them. Grouping
  shape: `Upper / Op1 / Ratio` style — matches PARAMETERS.md sections.
- Skip the VXN1 `sync.rs` analogue for now: VXN2's only sync params
  are `lfo1_sync` (rate partner: `lfo1_rate`) and `delay_sync` (rate
  partner: `delay_time`). Build a 4-entry static lookup inside the
  controller helper; defer a generic `sync` module until a third pair
  appears.
