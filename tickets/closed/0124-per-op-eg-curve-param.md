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

## Close-out (2026-06-24)

- `EgCurve { Exp = 0, Lin = 1 }` enum + `OpParams.eg_curve` (default `Exp`) in
  [eg.rs](../../vxn-2/crates/vxn2-dsp/src/eg.rs) /
  [op.rs:50](../../vxn-2/crates/vxn2-dsp/src/op.rs#L50). `eg::level_to_amp(level,
  curve)` and `EgState::cook(.., curve)` branch log-vs-square; cook passes
  `params.eg_curve` from both [op.rs:157](../../vxn-2/crates/vxn2-dsp/src/op.rs#L157)
  and [stack.rs:861](../../vxn-2/crates/vxn2-dsp/src/stack.rs#L861). The
  `EG_LOG_LEVELS` const is gone (`Exp` is the field default).
- State blob: `eg_curve_meta: AtomicU32` (1 bit/op, `N_EG_CURVES = N_OPS`),
  `eg_curve_shift`, `default_eg_curve_meta` (all `Exp`), `eg_curve_raw` /
  `set_eg_curve_raw` / `take_dirty_eg_curve`, `ParamView::eg_curve` (trait
  default `Exp` + `SharedParams` impl), seeded in `new`/`reset`/`mark_all_dirty`
  — mirroring `ks_curve_meta` ([shared.rs](../../vxn-2/crates/vxn2-engine/src/shared.rs)).
  `read_op` decodes `eg_curve: s.eg_curve(op)`.
- Blob **v15**: 4-byte EG-curve trailer appended after the KS trailer;
  `BLOB_VERSION 14 → 15`, `BLOB_EG_CURVE_LEN`, length/seed logic added; v≤14
  blobs decode 1:1 and seed every op to `Exp` (bit-identical migrated patch).
  Migration-test blob slicers (`rewrite_as_v10`, `v11_blob_seeds_legacy_ks_curves`,
  `snapshot_bytes_round_trip_is_bit_identical`) updated to strip the new trailer.
- Audio-thread mirror: `LocalParams.eg_curves` + `fetch_ui_changes` refresh +
  `ParamView::eg_curve` impl in
  [local.rs](../../vxn-2/crates/vxn2-clap/src/local.rs) so non-default `Lin`
  reaches the engine on the production path.
- Preset TOML: sparse `op{N}-eg-curve = "exp" | "lin"` (case-insensitive,
  default omitted) threaded through `decode_blob`/`encode_blob`/`read_preset`/
  `write_preset` in [preset.rs](../../vxn-2/crates/vxn2-engine/src/preset.rs).
- **No CLAP-param-table change** — patch-state-only like KS curve (decision:
  not automatable; UI exposure deferred to 0128).
- Tests: `eg_curves_round_trip_through_text` (preset.rs),
  `snapshot_bytes_round_trips_eg_curves` / `v14_blob_seeds_default_eg_curves` /
  `eg_curve_default_is_exp` / `set_eg_curve_raw_is_independent_per_op` (shared.rs),
  `shared_eg_curve_writes_reach_engine_op_params` (engine.rs),
  `exp_curve_is_log_lin_is_square` (eg.rs). dsp 184 / engine 205 lib green.
- No SIMD/CPU regression: curve selected in `cook` (control rate, scalar); the
  per-sample lane loop is untouched ([[vxn1-soa-match-defeats-simd]]).
