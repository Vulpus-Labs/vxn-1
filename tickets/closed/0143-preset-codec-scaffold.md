---
id: "0143"
product: monorepo
title: Extract shared preset-codec scaffold (value_for, Meta, PresetError)
priority: low
created: 2026-06-23
epic: E027
---

## Summary

Both engines hand-roll an almost-identical sparse-TOML preset
codec. The shared primitives diverge only in housekeeping;
the bodies differ legitimately (vxn-1 has a three-namespace
`PerformanceBody`, vxn-2 a flat `params + matrix` shape).
Extract the shared scaffold so a third synth — or the
not-yet-built vxn-2 preset epic — starts from it.

Duplicated, near-identical across vxn-1
`vxn-app/src/preset.rs` and vxn-2 `vxn2-engine/src/preset.rs`:

- `value_for(desc, v) -> toml::Value`
  (vxn-1 `:124-136`, vxn-2 `:213-225`).
- `Meta` struct, `PresetError` enum, `Header` struct,
  `SCHEMA` const, and the `from_toml_str` / `read_preset` /
  `write_preset` shape.

## Acceptance criteria

- [ ] `value_for`, `Meta`, `PresetError`, the `Header`/
      `SCHEMA` sparse-TOML scaffold, and the read/write shell
      live in one shared home (a new `vxn-preset` crate or a
      `vxn-core-app::preset` module — pick whichever keeps the
      dep graph acyclic).
- [ ] vxn-1 and vxn-2 codecs consume the scaffold and keep
      only their synth-specific body types.
- [ ] `cargo test --workspace` green; existing preset
      round-trip tests for both synths pass unchanged; the
      factory banks still load identically.

## Notes

Low urgency — pure dedup, no feature pressure until the vxn-2
preset epic (E007 lineage) is built; landing the scaffold
first would let that epic reuse it. Adding/editing factory
TOMLs won't recompile (memory `vxn2-include-dir-no-rerun`) —
touch `factory.rs` before `xtask install` when verifying
bank loads. Keep the per-synth body shapes untouched; only
the shared scaffold moves.

## Close-out (2026-07-01)

- Shared scaffold now lives in a new `vxn-preset` crate
  ([lib.rs](../../crates/vxn-preset/src/lib.rs)): `Meta`,
  `PresetError` (+`Display`/`Error`/`From<toml::de::Error>`),
  `Header`, `SCHEMA`, and `value_for` over a reduced
  `ScalarKind` bridge (the two engines' `ParamKind` types
  differ, so the shared renderer takes the reduced shape).
  Deps serde+toml only — no internal deps, graph stays
  acyclic. Wired into workspace members + `[workspace.dependencies]`
  in [Cargo.toml](../../Cargo.toml).
- vxn-1 consumes it: [vxn-engine/preset.rs:35](../../vxn-1/crates/vxn-engine/src/preset.rs#L35)
  `pub use vxn_preset::{Header, Meta, PresetError, SCHEMA}`
  (re-export keeps the `crate::preset::Meta` path stable for
  `factory.rs`, `preset_io.rs`, the lib re-export); local
  `value_for` maps `ParamKind`→`ScalarKind`. `PerformanceBody`
  stays local.
- vxn-2 consumes it: [vxn2-engine/preset.rs:112](../../vxn-2/crates/vxn2-engine/src/preset.rs#L112)
  same re-export; `PresetFile` + `MatrixRowFile` bodies stay
  local. `Meta` still field-for-field `vxn_core_app::PresetMeta`;
  store conversions unchanged.
- `value_for` enum branch unified to `v.round().max(0.0)`
  (vxn-1 previously omitted the floor; enum indices are never
  negative, so no behaviour change — guarded by round-trip +
  byte-parity tests).
- `cargo test --workspace` green (74 test-result-ok, 0 failed).
  vxn-1 engine 169 passed incl. `preset::tests` round-trip +
  the `app_writer_matches_engine_byte_for_byte` /
  `app_write_parses_on_engine` parity guards; vxn-2 engine
  `preset::tests` (`default_patch_round_trips_through_text`,
  `ks_curves_round_trip_through_text`,
  `eg_curves_round_trip_through_text`, `write_is_sparse`) pass
  unchanged. Factory banks load identically (no factory TOML
  or body-shape change).
- Incidental: fixed a pre-existing missing `use std::ops::Bound`
  in [vxn2-clap/src/lib.rs:649](../../vxn-2/crates/vxn2-clap/src/lib.rs#L649)
  test mod (untouched by this ticket; never compiled) that
  otherwise blocked the workspace test gate.
