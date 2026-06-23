---
id: "0124"
product: vxn-2
title: "Per-op EG curve mode param (Lin | Exp, default Exp)"
priority: high
created: 2026-06-23
epic: E026
depends: ["0123"]
---

## Summary

Second ticket of [E026](../../epics/open/E026-dx7-log-level-curve.md). Add a
per-operator `op{N}-eg-curve` selecting `Exp` (DX7 log curve, default) or `Lin`
(legacy square/linear) as an escape hatch. `Exp` is the corrected default from
0123; `Lin` preserves the old behavior for anyone who wants it on a given op.

## Design

Mirror the existing **patch-state-only** KS-curve plumbing (it is not a
CLAP-automatable param — see `op{N}-ks-l-curve`):

- `vxn2-dsp`: `EgCurve { Exp, Lin }` enum; `OpParams.eg_curve` (default `Exp`).
  `eg::level_to_amp` / cook branch on it (log vs square) for both EG L-values
  and operator output level. **Branch in `cook` only** (control rate, scalar) —
  never in the per-sample lane loop ([[vxn1-soa-match-defeats-simd]]).
- `vxn2-engine/shared.rs`: pack into the state blob like `ks_curve_meta`
  (1 bit/op), add a `ParamView` accessor, decode in `read_op` into
  `OpParams.eg_curve`. Engine setters mirror `set_ks_curve_raw`.
- `vxn2-engine/preset.rs`: TOML round-trip `op{N}-eg-curve = "lin" | "exp"`
  mirroring `ks_curve_key` / `ks_curve_from_name`; sparse-write (default `Exp`
  omitted).

## Acceptance criteria

- [ ] `OpParams.eg_curve` defaults `Exp`; cook selects log vs square per op.
- [ ] State-blob bit field + `ParamView` accessor + `read_op` decode + engine
      setter, mirroring `ks_curve_meta`.
- [ ] Preset TOML key `op{N}-eg-curve` round-trips (case-insensitive, sparse).
- [ ] No CLAP-param-table change unless we decide it should be automatable
      (default: patch-state-only like KS curve).
- [ ] Tests: TOML round-trip; cook picks the right curve per op; default `Exp`.
- [ ] No SIMD/CPU regression (curve selected at cook, not per sample).

## Notes

Supersedes the `EG_LOG_LEVELS` global const from 0123 — once the per-op flag
exists, the const goes away and `Exp` is the field default. EG is scalar per op
ticked at control rate (`voice.rs eg_tick`), so the per-op branch is free.
