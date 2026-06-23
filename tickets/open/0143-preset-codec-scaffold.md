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
