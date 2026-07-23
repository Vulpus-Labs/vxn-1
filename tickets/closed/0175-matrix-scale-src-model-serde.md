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
