---
id: "0237"
product: monorepo
title: "BlockRamp / CoeffRamp<K> primitives + PolyOtaLadder adoption + UpdateRate mapping"
priority: medium
created: 2026-08-02
epic: E043
depends: ["0226"]
---

## Summary

Second ticket of [E043](../../epics/open/E043-param-schema-control-vocabulary.md).
Name the one mechanism all three synths already use for hot-path parameter
motion — a linear per-sample ramp retargeted at a block edge — and adopt it
where drop-in bit-exact:

- `BlockRamp { cur, inc }`: `snap`, `retarget(target, steps)` (exact
  landing), `tick()` (one fadd), `value()`. This is vxn-2's `RampState`
  increment cell, vxn-1's ladder coeff ramp cell, and Smoothed-minus-the-
  one-pole, as one primitive.
- `CoeffRamp<const K: usize>`: K coefficients ramped in lockstep;
  generalises `PolyOtaLadder`'s `set_coeffs / prepare_ramp / tick_coeffs`
  protocol (retarget at **base** rate, consume at **OS** rate — the caller's
  contract, type-assisted by the 0226 rate newtypes at the cook boundary).
- Adopt `CoeffRamp` inside
  [poly/ladder.rs:216-257](../../vxn-1/crates/vxn-dsp/src/poly/ladder.rs#L216-L257)
  — identical linear-increment arithmetic, so bit-exact.
- Map vxn-1 `Glide{Snap,Block,PerSample}`
  ([smoothing.rs:155](../../vxn-1/crates/vxn-engine/src/smoothing.rs#L155))
  and vxn-1b Motion/Fx classifications onto the shared `UpdateRate` names —
  rename-level. vxn-2's `RampState`/`PitchSmoother` are documented against
  the taxonomy, **not rewritten** (the EG-fold and 16-sample quantum are
  vxn-2's sound).

## Acceptance criteria

- [ ] Primitives in `vxn-core-dsp::control` with exact-landing + snap-epsilon
      unit tests.
- [ ] `PolyOtaLadder` on `CoeffRamp`: ladder exact-landing test
      ([ladder.rs:356](../../vxn-1/crates/vxn-dsp/src/poly/ladder.rs#L356)),
      vxn-1 baseline, vxn-1b parity all byte-identical.
- [ ] asm-check: `poly/ladder` monomorph NEON counts unchanged — this is the
      one hot-path change in the epic.
- [ ] Zero golden movement anywhere (zipper suites, param_audibility green).

## Notes

Smoothing *policies* stay per-synth by design — shared vocabulary, per-synth
voicing. Related: [[vxn2-level-mod-pipeline]] (the RampState tick formula is
frozen), [[vxn1-envelope-soa-not-worth-it]].
