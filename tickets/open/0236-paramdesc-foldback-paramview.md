---
id: "0236"
product: vxn-2
title: "ParamDesc/Taper/ParamKind fold-back onto vxn-core-app + shared ParamView trait"
priority: medium
created: 2026-08-02
epic: E043
depends: ["0222"]
---

## Summary

First ticket of [E043](../../epics/open/E043-param-schema-control-vocabulary.md).
[vxn2-engine/src/params.rs](../../vxn-2/crates/vxn2-engine/src/params.rs)
defines its own `ParamDesc`/`Taper`/`ParamKind` and bridges to
`vxn-core-app`'s via `to_core()` (:683) + a mirrored `CORE_PARAMS` table
(:718) — a live duplication whose bridge proves the types isomorphic. Fold it
back:

- vxn-2's `PARAMS` table declared directly in `vxn_core_app` types; local
  descriptor types + `to_core`/`CORE_PARAMS`/`core_desc_for_clap_id` deleted.
  Field deltas (e.g. `Int { unit }`) reconciled by extending `vxn-core-app`
  **additively** — non-breaking for vxn-1/1b/3.
- `pub trait ParamView { fn get(&self, id: usize) -> f32; }` added to
  `vxn-core-app` next to `ParamModel`;
  [shared.rs:258](../../vxn-2/crates/vxn2-engine/src/shared.rs#L258) becomes
  `Vxn2ParamView: vxn_core_app::ParamView` extension subtrait (the defaulted
  matrix/KS/EG accessors name vxn2 types and stay); `EngineParams::snapshot_from`
  re-bounds on the subtrait.

## Acceptance criteria

- [ ] Grep: exactly one `ParamDesc` definition repo-wide (vxn-core-app); no
      `to_core`.
- [ ] vxn-2 render hash, controller tests, param_sweep, editor round-trip
      (descriptor/taper JSON) all unchanged.
- [ ] vxn-1/1b/3 builds untouched by the additive vxn-core-app extension.

## Notes

`ParamView` is the seed of the shared control-rate model — vxn-1's
`ParamSmoother::tick_block` and vxn-1b's `FxParams::from_params` remain
per-synth *policies* over it; nothing forces them onto vxn-2's snapshot shape.
