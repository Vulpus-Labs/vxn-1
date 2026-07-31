---
id: "0216"
product: vxn-1b
title: "Per-layer mod matrix — private 16-slot matrix per synth, 32 depth params"
priority: high
created: 2026-07-31
epic: E039
depends: ["0214"]
---

## Summary

Give each `Synth` ([[0214]]) its **own private 16-slot mod matrix**. Today VXN1b
has one matrix: topology in blob state, 16 automatable depth params
([vxn1b-engine/src/matrix.rs](../../vxn-1b/crates/vxn1b-engine/src/matrix.rs),
[params.rs:216-231](../../vxn-1b/crates/vxn1b-engine/src/params.rs#L216-L231)).
Per [ADR 0002](../../vxn-1b/adrs/0002-vxn1b-dual-layer.md) §4.

## Design

- **Two matrix instances**, one per `Synth`. No pooled slots, no cross-layer
  "Both" tag, **no cross-layer routing** — a slot's source and dest are always
  within its own layer.
- **Sources all per-layer**: Env1, Env2, LFO1, **LFO2** (VXN1b has no global
  LFO), plus per-voice/controller sources (Velocity, Key, ModWheel, PitchWheel,
  Aftertouch, NoteRandom). LFO2's optional cross-layer sync is [[0217]].
- **Params**: matrix depths double to **32** — `Layer1 MatrixSlot0..15 Depth`
  + `Layer2 MatrixSlot0..15 Depth`, each bipolar `[-1,1]`. Extend the `ParamId`
  enum + descriptor table; keep the flat CLAP-id layout clean
  ([[vxn1-id-stability-dropped]]).
- **Topology blob ×2**: two 16-slot topology records (source/dest/curve/scale_src)
  in `PluginState`, one per layer.
- **Eval private to each synth**: matrix runs per-voice within each `Synth`; a
  voice only sees its own layer's matrix. No shared source/dest resolution.

## Acceptance

- Each `Synth` has an independent, separately programmable 16-slot matrix.
- 32 depth params exposed (16/layer); topology stored ×2 in blob.
- Sources/dests resolve within-layer only; no cross-layer route is expressible.
- Round-trip test: distinct matrices per layer survive save/reload.
- `cargo test -p vxn1b-engine` green; allocation-free callback.

## Close-out (2026-08-01)

Implemented as **full patch doubling** (ADR 0002 §4, per the user's scope call),
not just the matrix — the per-layer matrix falls out of the two-layer surface.

- **Two-layer CLAP map over an unchanged inner table.** Kept per-synth
  `ParamId`/`PARAMS`/`Params` as-is; added an outer map in
  [params.rs](../../vxn-1b/crates/vxn1b-engine/src/params.rs): `PATCH_PARAMS` (64
  per-layer, incl. 16 matrix depths) + `GLOBAL_PARAMS` (32), laid out
  `[L1 patch][L2 patch][globals]`, `TOTAL_PARAMS = 2·64 + 32 = 160`. `clap_ref`,
  `patch_clap_id`/`global_clap_id`/`clap_id_of`, `clap_module` (Upper/Lower).
  Tests `patch_and_global_partition_every_param`, `clap_ref_layout_is_l1_l2_globals`,
  `module_labels_split_by_layer`.
- **32 depth params (16/layer); per-synth matrix.** Engine
  [set_param/param](../../vxn-1b/crates/vxn1b-engine/src/engine.rs#L165) route
  L1→synth 0, L2→synth 1, globals→both. `matrix_mut(Layer)`. Independence proven:
  `engine::tests::set_param_mirrors_slot_depth_into_matrix` (a L1 depth edit
  leaves L2's slot at its factory value; a L2 edit is private).
- **Within-layer only.** Sources/dests are structurally per-synth — no cross-layer
  route is expressible (each `Synth` evaluates only its own `MatrixTable`).
- **Topology ×2 in blob + round-trip.** [state.rs](../../vxn-1b/crates/vxn1b-engine/src/state.rs):
  `LayerState` (single-layer helper) + `PluginState{layers:[LayerState;2]}`,
  MAGIC `VX1B`, **VERSION 1→2** (no pre-release migration).
  `roundtrips_both_layers_independently` (distinct params + a slot layer 1 leaves
  inert survive), `blob_length_is_two_full_layers`. `SharedParams` holds
  `[MatrixTable;2]` + 160 CLAP values, maps via `clap_ref`:
  `shared::tests::snapshot_restore_round_trips_and_flags_reload` proves L2's
  private matrix edit lands on L2 only.
- **Consumers wired.** Clap `get_info` sets the Upper/Lower module tag; ui-web
  `PATCH_COUNT` const now = `vxn1b_engine::PATCH_COUNT` (64) — the JS
  [dispatch.js](../../vxn-1b/crates/vxn1b-ui-web/assets/dispatch.js)
  `paramIdByNameAtLayer` (lower = upper + patchCount) already matches the layout.
- **Interim: presets stay single-layer** (LayerState) — load sets Layer 1 +
  resets Layer 2; two-layer presets + KeyState persistence are 0221.
- **Green:** `vxn1b-engine` 115 lib + 8 integration (incl. `alloc_free`, parity);
  `vxn1b-clap` 5; `vxn1b-ui-web` 5 Rust + 159 vitest. Clean build (no warnings).
