---
id: "0205"
product: vxn-1b
title: "Wire matrix slot-depth params to the evaluator (depth sync + default-patch seed)"
priority: high
created: 2026-07-27
epic: E036
depends: []
---

## Summary

Slot depth is stored in **two** places that never reconcile, so the automatable
depth params are decorative — a correctness bug surfaced while building the
persistence codec ([0203](0203-vxn1b-persistence.md), part of
[E036](../../epics/open/E036-vxn1b-matrix-engine.md)).

- **Param table** — `MatrixSlot0Depth`…`MatrixSlot15Depth` are CLAP params
  ([params.rs:186-203](../../vxn-1b/crates/vxn1b-engine/src/params.rs#L186-L203)),
  seeded to **0.0** by `Params::default()`
  ([params.rs:377](../../vxn-1b/crates/vxn1b-engine/src/params.rs#L377)).
- **MatrixTable** — each `MatrixSlot` carries its own `depth`
  ([matrix.rs:232](../../vxn-1b/crates/vxn1b-engine/src/matrix.rs#L232)), seeded
  **non-zero** by `default_patch()` (Env2→Amp @ 1.0, LFO1→Pitch @ 0.05/12 —
  [matrix.rs:324-344](../../vxn-1b/crates/vxn1b-engine/src/matrix.rs#L324-L344)).

The evaluator reads the **MatrixTable** copy, not the param
([eval.rs:175-182](../../vxn-1b/crates/vxn1b-engine/src/eval.rs#L175-L182)).
Two consequences:

1. **Startup mismatch.** A fresh engine has `matrix.slots[0].depth == 1.0` but
   `params[MatrixSlot0Depth] == 0.0`. The sound is right (uses the matrix) but a
   host reading the param sees the wrong value.
2. **Dead automation.** `set_param`
   ([engine.rs:81-86](../../vxn-1b/crates/vxn1b-engine/src/engine.rs#L81-L86))
   writes `params` only; it never touches `matrix.slots[i].depth`, so a host
   automating a slot depth changes **nothing** audible.

ADR 0001 §5 makes the depth *param* the authority. The engine must honour that:
the param drives the evaluator, and the default-patch seed depths must live in
the param defaults (not only in `default_patch()`).

## Design

Two-part fix, both in `vxn1b-engine`:

- **Sync on set.** In `set_param`, when `id` is a slot-depth param, mirror the
  value into `matrix.slots[i].depth` (alongside the existing envelope re-cook).
  Add a `ParamId::slot_depth_index(id) -> Option<usize>` (inverse of
  `slot_depth`) so the branch is a table lookup, not a range test.
- **Reconcile the seed.** Make the default-patch depths and the param defaults
  agree at construction. Prefer: give the three seeded slots' depth *params*
  their real defaults (Amp 1.0, vibrato 0.05/12, key-track 0.0) in the
  descriptor table, and drop `depth` from `default_patch()`'s literals (or have
  the engine seed `matrix.depth` from `params.slot_depth(i)` right after
  `default_patch()` — one loop, same as the load path in
  [state.rs](../../vxn-1b/crates/vxn1b-engine/src/state.rs) /
  [preset.rs](../../vxn-1b/crates/vxn1b-engine/src/preset.rs)). Keep the two
  representations reconciled in exactly one place.

Watch the parity target: `DEFAULT_VIBRATO_DEPTH` /
`KEY_CUTOFF_UNITY_DEPTH` and the 0202 render-parity tests must still pass — the
seeded depths that reproduce VXN1's default sound cannot drift.

## Acceptance criteria

- [ ] Automating a slot-depth param changes the evaluator's output: a test sets
      `MatrixSlot2Depth` via `set_param` and asserts the Pitch dest total moves.
- [ ] Fresh engine: `params[MatrixSlotNDepth] == matrix.slots[N].depth` for all
      16 slots (no startup mismatch) — assert in a test.
- [ ] The default patch still sounds like VXN1: existing 0202 render-parity /
      `default_patch` tests stay green (Amp 1.0, vibrato 0.05 st, key-track off).
- [ ] Depth remains stored/authored in exactly one place at rest; the other is
      derived — no third code path can desync them.

## Notes

- Feeds [0204](0204-vxn1b-clap-shell-bundle.md): the CLAP shell's host param
  apply routes through `set_param`, so this must land for automation to work in
  a DAW.
- Persistence (0203) already treats params as the depth authority and re-seeds
  the matrix on load, so no format change is needed — this is engine wiring only.
- Related design: [[vxn2-level-mod-pipeline]] (VXN2's combined depth/EG ramp)
  and [[vxn2-e006-review-remediation]] (Stack-vs-engine ramp codegen lesson) —
  VXN2 hit the same "who owns the ramped value" question.

## Close-out (2026-07-27)

- **Live depth automation.** `set_param`
  ([engine.rs:87-97](../../vxn-1b/crates/vxn1b-engine/src/engine.rs#L87-L97))
  now mirrors a slot-depth edit into `matrix.slots[i].depth` — the copy the
  evaluator ([eval.rs](../../vxn-1b/crates/vxn1b-engine/src/eval.rs)) and the
  bank amp path ([bank.rs:610-634](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L610-L634))
  read — using a clamped read-back so the mirror can't drift from the param.
  `engine::tests::set_param_mirrors_slot_depth_into_matrix` (mirror + clamp) and
  `zeroing_amp_slot_depth_via_param_silences_note` (param → matrix → DSP end to
  end: zeroing the default Env2→Amp depth kills the VCA route and silences the
  note).
- **No startup mismatch.** `Engine::new`
  ([engine.rs:62-72](../../vxn-1b/crates/vxn1b-engine/src/engine.rs#L62-L72))
  seeds the 16 depth params from `default_patch()` once, so param and matrix
  agree from frame zero — `engine::tests::fresh_engine_params_match_matrix_depths`.
- **Parity intact.** All `render::tests` (default-patch vibrato/amp/key-track
  parity) and `matrix::tests::default_patch_*` stay green — `DEFAULT_VIBRATO_DEPTH`
  / `KEY_CUTOFF_UNITY_DEPTH` unchanged.
- **Single authority.** `default_patch()` authors the seed depths; the param
  table is seeded from it and is thereafter the authority, with `set_param` the
  only mutating path (it mirrors). Added `ParamId::slot_depth_index`
  ([params.rs:231-241](../../vxn-1b/crates/vxn1b-engine/src/params.rs#L231-L241))
  as the inverse of `slot_depth`; `params::tests::slot_depth_index_is_the_inverse_of_slot_depth`
  guards it and the `then` vs `then_some` usize-underflow trap caught in review.
- 86 tests pass (`cargo test -p vxn1b-engine --lib`), clippy clean. Commit `09271d2`.
