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
</content>
