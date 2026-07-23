---
id: "0175"
product: vxn-2
title: Mod-matrix scale source — data model + patch/state serde
priority: medium
created: 2026-07-03
epic: E033
depends: []
---

## Summary

Add a `scale_src: SourceId` field to `MatrixSlot` (default `SourceId::None`) and
carry it through every persistence path — TOML presets, `default_patch`, and the
binary `clap.state` blob — with back-compat defaulting. This is the foundation
ticket: it introduces the field and its serde but changes **no** audio
behaviour, since `None` is already the eval identity.

## Acceptance criteria

- [ ] `MatrixSlot` gains `scale_src: SourceId`, defaulting to `SourceId::None`;
      all constructors / `default_patch` set it.
- [ ] TOML round-trips a per-slot `slotN-scale-src` key using `SOURCE_NAMES`
      (kebab wire names); absent key → `None`; unknown name → `None`.
- [ ] Binary state blob writes the field and reads it with a version bump; a
      pre-epic state fixture (no field) loads with `scale_src = None`.
- [ ] No change to `eval_dests` yet — render output is unchanged (`None` is
      identity). Existing render-hash / regression tests still pass.
- [ ] Unit tests: TOML round-trip, absent→None, unknown→None, blob back-compat
      read.

## Notes

`scale_src` is topology (like `source`/`dest`/`curve`), **not** a
`clap.params` entry — no automatable id is added. Follow the exact serde pattern
already used for the slot `source`/`dest`/`curve` fields in `preset_io.rs`.
Touchpoints: `matrix.rs` (struct + defaults), `preset_io.rs` / `preset.rs`
(TOML), `default_patch.rs`, `shared.rs` (state blob + version). See
[E033](../../epics/open/E033-matrix-scale-source.md) for the locked design.

## Close-out (2026-07-23)

- `MatrixSlot.scale_src: SourceId` (default `None`) added; all constructors +
  [default_patch.rs](../../vxn-2/crates/vxn2-engine/src/default_patch.rs) set it.
  Threaded through `MatrixRowRaw`, app `MatrixRow`, and `EngineParams`.
- TOML `scale-src` kebab key ([preset.rs](../../vxn-2/crates/vxn2-engine/src/preset.rs)):
  sparse (omitted when `none`), absent→None, unknown→None with a warning.
  Tests `preset::tests::matrix_scale_src_round_trips_through_text`,
  `…_omitted_when_none`, `…_absent_and_unknown_degrade_to_none`.
- Binary state: packed into the previously-reserved low-byte bits of the slot's
  `matrix_meta` word ([shared.rs](../../vxn-2/crates/vxn2-engine/src/shared.rs))
  — **no blob version bump**; pre-E033 blobs (bits clear) decode to `None`.
  Tests `shared::tests::matrix_scale_src_survives_blob_round_trip`,
  `…::pre_e033_blob_decodes_scale_src_none`.
- No `eval_dests` behaviour change here; all engine render/regression tests
  still pass. Landed in `27d8823`.
