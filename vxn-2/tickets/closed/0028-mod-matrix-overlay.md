---
id: "0028"
title: Mod matrix overlay (16 slots, source / dest / depth / smoothing)
priority: high
created: 2026-06-06
closed: 2026-06-07
epic: E003
---

## Summary

Build the 16-row mod-matrix overlay. Each row exposes:

- `source` enum (11 variants — see PARAMETERS.md mod matrix table).
- `dest` enum (per-op + global + stacking + FX destinations,
  ≈ 40 variants).
- `depth` float [-1.0, +1.0]. Slots 1-8 depth is CLAP-automatable;
  slots 9-16 depth is patch state (rides the custom event path).
- `curve` enum {lin, exp, log, bipolar}.
- `active` bool — empty rows render greyed; active rows render
  their tile in colour.

Slots 1-8 carry an "automatable" badge so users park their DAW-
driven routings there (PARAMETERS.md §"CLAP exposure" note).

## Acceptance criteria

- [ ] Overlay opens via the `mod-matrix` button on the gmod-row;
      Escape / outside-click / button re-click closes it.
- [ ] 16 rows × 5 fields. Source / dest / curve are
      `<select>` elements wired to dispatch
      `set_matrix_row { slot, source, dest, depth, curve, active }`
      via the `Vxn2UiCustom::SetMatrixRow` custom event. Depth +
      active dispatch the same opcode (single source of truth per
      row).
- [ ] Slots 1-8 depth ALSO dispatches plain
      `set_param { id: <mtx{1..8}_depth_clap_id>, plain: ... }`
      so host automation reads correctly. The custom event path
      keeps the row's source / dest / curve / active in sync;
      depth flows through both paths (CLAP wins on automation).
- [ ] Slots 9-16: depth widget styled with no "automatable"
      badge; only the custom event path fires.
- [ ] `applyViewEvents` for
      `Vxn2ViewCustom::MatrixRowChanged { slot, source, dest,
      depth, curve, active }` re-renders that row.
- [ ] Layer awareness: matrix overlay shows the current
      `edit_layer`'s rows. Layer toggle re-renders from
      `vxn.matrix.upper` / `vxn.matrix.lower` (controller pushes
      both tables on `EditorReady` and on `edit_layer` change).
- [ ] Edit a slot in a host: pick `lfo1` source, `op1_level`
      dest, depth 0.5 — assert the engine's audible output
      reflects the new routing within one tick.
- [ ] Removing a row (set `active` false) silences the route on
      the engine within one tick.

## Notes

- Sources / destinations are static enums; build the lists in JS
  by reading from `params.json` or a small companion JSON the
  engine emits at build time. Don't hand-maintain — the engine
  owns the truth list (see `vxn2-engine::matrix`).
- Active toggle is the simplest gate; the engine's matrix
  evaluator already skips rows with depth=0 (or active=false —
  confirm in 0008's impl). If the engine still evaluates inactive
  rows, file a follow-up — not in this ticket's scope.
- Depth path divergence (CLAP for slots 1-8, custom-only for
  9-16) is the only ugly part. Document the seam in the JS
  comments and the Rust `Vxn2UiCustom::SetMatrixRow` handler so
  a future reader sees why depth has two code paths.
- Curve options are display-only for now — the engine's matrix
  applies a single linear curve regardless (per ADR 0001 §6 v2
  scope). The UI fields exist so presets can carry the user's
  intent through to a future engine pass. State this clearly in
  the overlay tooltip.
- Source `voice_idx` / `voice_spread` / `voice_rand` are
  per-voice sources tied to the stacking macros (PARAMETERS.md
  §"Voice stacking"). Pickable like any other source.
- Mod matrix slot tables are 16 × 2 layers = 32 rows of state in
  the patch blob. State serialisation goes through
  `ParamModel::snapshot_bytes` — already wired by 0017 in E002,
  no engine work needed in this ticket beyond confirming the
  blob round-trip catches matrix-row writes.
