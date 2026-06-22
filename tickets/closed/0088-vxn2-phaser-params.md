---
id: "0088"
product: vxn-2
title: "Phaser CLAP params (host-automation only) + preset round-trip"
priority: medium
created: 2026-06-22
epic: E025
depends: ["0087"]
---

## Summary

Second ticket of [E025](../../epics/open/E025-vxn2-fx-tabs-phaser.md).
Append five phaser params to vxn-2's flat CLAP param table as
host-automation params, decode them into a `PhaserParams` struct, and
confirm preset round-trip. **Not** added as mod-matrix destinations.

## Design

Param table: `vxn-2/crates/vxn2-engine/src/params.rs`. Current FX
block is Delay (ids 169–174) + Reverb (175–179) inside the patch-level
globals; `N = 196` total (`PARAMS[196]`).

**Append** the phaser params at the **end** of the table — new ids
196–200, `N` → 201:

- `phaser-on`    (bool, default 0)
- `phaser-rate`  (Hz, e.g. 0.05–8, default ~0.4)
- `phaser-depth` (0..1, default ~0.6)
- `phaser-feedback` (-1..1 or 0..1 mapped to ±0.9, default ~0.3)
- `phaser-mix`   (0..1, default ~0.5)

Appending (not inserting into the FX block) keeps existing
delay/reverb param ids stable, so saved DAW automation/sessions
survive. Add `OFF_PHASER` section offset alongside `OFF_DELAY` /
`OFF_REVERB` (params.rs:626).

Decode: add a `PhaserParams` struct (mirror `StereoDelayParams` /
`FdnReverbParams`) and a decode arm in `shared.rs` (~line 1315, where
delay/reverb are read via `shared.get(pb + OFF_...)`), reading the five
phaser ids and clamping/mapping into struct fields.

Mod matrix: **do not** touch `matrix.rs` — no `DestId` variant, no
`DEST_NAMES` entry. Host-automation only, per epic.

Presets: name-keyed sparse TOML ([[vxn2-preset-system]]). New
`phaser-*` keys default-fill on load; old presets load with phaser
off. No factory bank migration.

## Acceptance criteria

- [ ] Five `phaser-*` ids appended; `N`/`PARAMS` length bumped to 201;
      delay/reverb ids unchanged.
- [ ] `OFF_PHASER` offset added; `PhaserParams` struct defined.
- [ ] `shared.rs` decodes the five ids into `PhaserParams` with sane
      clamps/ranges.
- [ ] `grep -i phaser vxn-2/crates/vxn2-engine/src/matrix.rs` returns
      nothing.
- [ ] Loading a pre-epic preset yields `phaser-on = false`; saving +
      reloading round-trips the phaser values.
- [ ] `cargo test -p vxn2-engine` passes.

## Notes

Param decode flows per-block into the `EngineParams` snapshot; 0089
fans `PhaserParams` to `phaser.set_params(...)` in
`apply_block_params()`.

## Close-out (2026-06-22)

- Five `phaser-*` params appended at the table tail (ids 196–200);
  `N_PATCH_LEVEL` 27→32, `TOTAL_PARAMS` 196→201; delay/reverb ids unchanged.
  `phaser-on` default off, `phaser-feedback` range -0.9..0.9.
- `OFF_PHASER = 27` added; `module_for_patch` labels the block `Global /
  Phaser`. `PhaserParams` lives in `vxn2-dsp::phaser` (mirrors
  `StereoDelayParams`); `shared.rs` decodes the five ids into
  `EngineParams.phaser`.
- Blob migration: `BLOB_VERSION` 13→14, `N_PHASER_PARAMS_V14 = 5`,
  `LEGACY_V13_PARAM_COUNT` added, the three TOTAL-anchored legacy counts
  re-anchored off it, `13 =>` arm added, and the two test rewriters drop the
  phaser tail. Pre-epic presets load with `phaser-on = false`; round-trip
  verified by the migration suite.
- Mod matrix untouched: `grep -i phaser matrix.rs` → nothing.
- `cargo test -p vxn2-engine` passes (param/shared goldens updated).
