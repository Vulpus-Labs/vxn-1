---
id: E043
product: monorepo
title: "Param schema fold-back + control-rate vocabulary — one ParamDesc, shared ParamView, BlockRamp/CoeffRamp primitives (bit-exact)"
status: open
created: 2026-08-02
---

> vxn-2 carries a duplicate `ParamDesc`/`Taper`/`ParamKind` in
> [params.rs](../../vxn-2/crates/vxn2-engine/src/params.rs) with a `to_core()`
> bridge onto `vxn-core-app` — a live fork of the shared schema proving the
> types are isomorphic. Separately, three synths express "linear per-sample
> ramp retargeted at a block edge" three ways (vxn-2 `RampState` increments,
> vxn-1 ladder coeff ramps, `Smoothed`-as-ramp). This epic folds the schema
> back and names the shared control-rate vocabulary: `UpdateRate` taxonomy +
> `BlockRamp`/`CoeffRamp<K>` mechanism. Smoothing **policies** (Glide tables,
> RampState EG-fold, PitchSmoother quantum, MotionSmoother) stay per-synth —
> mechanism shared, voicing not.

## Goal

- `vxn-core-app` owns the only `ParamDesc`/`Taper`/`ParamKind`; vxn-2's
  `to_core` bridge is deleted; `ParamView { get }` lives in `vxn-core-app`
  with vxn-2's version as an extension subtrait.
- `vxn-core-dsp::control` gains `BlockRamp` + `CoeffRamp<K>`; `PolyOtaLadder`
  adopts `CoeffRamp` (identical linear-increment arithmetic — bit-exact).
- vxn-1 `Glide` / vxn-1b Motion+Fx classifications map onto `UpdateRate`
  names (rename-level change).

## Planned tickets

Independent of E041/E042; needs E040/0222+0226. Chain: **0236, 0237** (independent).

- [ ] **0236** — ParamDesc fold-back + shared `ParamView` + `Vxn2ParamView` subtrait.
- [ ] **0237** — `BlockRamp`/`CoeffRamp<K>` + `PolyOtaLadder` adoption + `UpdateRate` mapping.

## Acceptance

- Zero golden movement: all three baselines/parity oracles, zipper suites,
  param_sweep, param_audibility, ladder exact-landing ramp tests green.
- asm-check on `poly/ladder` monomorphs unchanged (0237 is the one hot-path
  change in this epic).
- Grep: no `to_core`, no duplicate `ParamDesc` definition outside vxn-core-app.
