---
id: "0203"
product: vxn-1b
title: "Persistence: sparse TOML matrix table + packed binary clap.state topology"
priority: medium
created: 2026-07-25
epic: E036
---

## Summary

Round-trip the matrix **topology** through both persistence paths, following the
VXN2 conventions ([ADR 0001](../../vxn-1b/adrs/0001-vxn1b-overall-design.md) §6;
VXN2 [ADR 0009](../../vxn-2/adrs/0009-matrix-scale-source.md) "Persistence").
Slot **depths** already ride the normal param blob (0200) — this ticket handles
`source`/`dest`/`curve`/`scale_src`.

- **TOML preset:** sparse `[[matrix]]` entries with kebab keys
  (`source`/`dest`/`curve`/`scale-src`); inactive slots omitted; an absent key or
  unknown name decodes to `None`. Name-keyed preset per VXN1 ADR 0005; fork
  VXN1's [preset_io.rs](../../vxn-1/crates/vxn-engine/src/preset_io.rs).
- **Binary `clap.state`:** pack each slot's active-bit + source/dest/curve/scale
  bytes (VXN2 ADR 0009 byte layout). Fork VXN1's
  [state.rs](../../vxn-1/crates/vxn-engine/src/state.rs). A pre-topology/empty
  blob decodes to all-`None` slots (back-compat default read).

## Acceptance criteria

- [ ] TOML round-trips all 16 slots' topology; inactive slots are omitted
      (sparse); absent/unknown keys decode to `None`.
- [ ] `clap.state` round-trips slot topology; depths continue via the param blob.
- [ ] A blob lacking matrix topology loads with all slots `None` (default read).
- [ ] Tests: TOML round-trip, sparse omission, unknown-name→None, binary
      round-trip, back-compat default.

## Notes

- Depths are params, not topology — do not double-encode them in the TOML matrix
  table or the state topology bytes.
- Reuse the shared `vxn-preset` / `vxn-core-clap` primitives where they fit; the
  empty-state-load contract (0196) must hold — an empty blob returns `false` from
  CLAP `load`, not a silent accept.
- Depends on 0201 (model), 0202 (evaluator consumes it). Feeds 0204 (CLAP state).

## Close-out (2026-07-27)

- **TOML round-trip + sparse omission + unknown→None.**
  [preset.rs](../../vxn-1b/crates/vxn1b-engine/src/preset.rs) forks the sparse
  name-keyed codec onto the shared `vxn-preset` scaffold (`Meta`/`Header`/
  `SCHEMA`/`value_for`/`PresetError`). `[[matrix]]` rows carry
  `source`/`dest`/`curve`/`scale-src` kebab names for routed slots only;
  `curve` defaults `lin`, `scale-src` omitted when `none`. Absent/unknown names
  and unknown param keys degrade to `None`/default with a non-fatal warning.
  Tests: `preset::tests::topology_round_trips_through_text`,
  `write_is_sparse_and_omits_inactive_slots`,
  `unknown_source_warns_and_leaves_slot_inert`,
  `unknown_scale_src_degrades_to_none_with_warning`, `absent_curve_and_scale_default`,
  `slot_out_of_range_warns_and_skips`, `enum_label_is_case_insensitive`,
  `value_clamps_on_read`, `schema_mismatch_is_typed_error`, `malformed_toml_is_error`.
- **Binary `clap.state` round-trip; depths via the param blob.**
  [state.rs](../../vxn-1b/crates/vxn1b-engine/src/state.rs) packs magic `VX1B` +
  version + `f32` param block + a 5-byte `[active, source, dest, curve, scale]`
  record per slot (VXN2 ADR 0009 layout). Depth is never in the topology bytes —
  `state::tests::depths_are_not_double_encoded_in_topology` asserts the blob size
  leaves no room for it. Round-trip: `roundtrips_params_and_topology`.
- **Back-compat default read.** A blob ending after the param block decodes with
  all-`None` slots (`blob_without_topology_reads_all_none`); a partial topology
  reads present slots and defaults the rest (`partial_topology_reads_present_slots_and_defaults_rest`);
  a truncated record is a hard error (`truncated_slot_record_is_an_error`).
- **0196 empty-state contract preserved.** An empty/bad-magic/bad-version blob is
  a hard `read` error, not a silent accept (`empty_blob_is_an_error`,
  `rejects_bad_magic_and_version`) — the CLAP layer maps this to a `false` load in
  0204. Depth authority is the param block: both readers re-seed
  `matrix.slots[i].depth` from params on load, so topology is render-ready.
- **No double-encode.** Depth appears once, as the `matrix_slotN_depth` param key;
  `[[matrix]]` rows have no `depth` field (`depth_is_not_duplicated_in_matrix_rows`).
- 20 new tests, all green (`cargo test -p vxn1b-engine --lib`); clippy clean.
  Shipped in commit `2853a08`.
- **Follow-up filed:** [0205](0205-vxn1b-slot-depth-param-sync.md) — the engine
  doesn't yet sync slot-depth params into `matrix.slots[i].depth` (the evaluator's
  read), so live depth automation is dead and startup params/matrix disagree.
  Persistence sidesteps it (params = authority, load re-seeds); the engine wiring
  is 0205, feeding 0204.
</content>
