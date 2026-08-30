---
id: "0334"
product: monorepo
title: "Share the evaluator — one dest-major lane loop, const-generic over lane count"
priority: medium
created: 2026-08-30
epic: E049
depends: ["0328", "0331", "0333"]
---

## Summary

The last and largest mechanism move: one evaluator under both synths.

vxn-1b's [`eval_dests_bank`](../../vxn-1b/crates/vxn1b-engine/src/eval.rs) is
already the target shape — dest-major SoA (`[[f32; L]; N_DESTS]`), const-generic
over lane count, every per-route decision hoisted above the lane loop, which
LLVM contracts to NEON. vxn-2's `eval_dests` is lane-major and compiles to
entirely scalar code.

**This ticket cannot start until [0328](0328-matrix-dest-major-lane-accumulators.md)
lands.** Sharing an evaluator across two different memory layouts is not
possible, and 0328 is where vxn-2 converges on vxn-1b's.

## Design

Take vxn-1b's bank form as the canon and make it generic over `MatrixRoster`.
vxn-2 supplies `L = STACK_LANES`; vxn-1b supplies its `RenderBank::LANES`.

Keep the scalar per-voice form too, as the reference implementation the banked
one is proved against — vxn-1b's parity test is the model, and
[0331](0331-matrix-golden-vector-harness.md) generalises it to run every case
through both.

Preserve, and do not "clean up":

- **The association.** vxn-1b's inner loop is `shape(v) * (gain * scale[l])`,
  grouped that way deliberately — the scalar form multiplies by a `slot_gain`
  that has already folded `topology * scale`, and regrouping rounds differently.
  Its comment says so; the parity test enforces it.
- **Hoisting discipline.** Polarity, shape, scale source, scale polarity and
  scale bend all dispatch outside the lane loop. Collapsing any of them back
  inside costs the vectorisation — [[vxn2-matrix-hot-loop-lessons]] measured a
  50% regression from exactly one such call.
- **`clamp_unit` over `f32::clamp`**, and the branch in `shape_log` over
  `copysign`. Both measured, both counter-intuitive, both documented.

## Acceptance criteria

- [ ] One evaluator in `vxn-core-matrix`, used by both synths; neither carries
      its own lane loop.
- [ ] Every [0331](0331-matrix-golden-vector-harness.md) case passes through
      both the scalar and banked paths, bit-exactly.
- [ ] Emitted asm for the banked evaluator contains NEON 4-wide arithmetic under
      both synths' lane counts. Grep the **mnemonic** for `.4s`, not the operands
      — see [[vxn1-neon-grep-pitfall]]; `grep 'v\d+\.4s'` returns nothing on
      genuinely vectorised ARM64 and would make success look like failure.
- [ ] Null test against the pre-ticket render passes: difference peak
      ≤ −100 dBFS (see [E049](../../epics/open/E049-shared-matrix-routing.md)
      §"The bar"). Re-capture the render hashes if they moved, and say so.
- [ ] Benchmarks: vxn-2's `matrix_eval_*` improve (it is gaining vectorisation);
      vxn-1b's hold. A vxn-1b regression means the generic form cost it
      something the monomorphic one didn't — measure and report before accepting.

## Notes

- Highest-risk ticket in the epic. Both synths have render-hash baselines by
  this point (vxn-1b's lands in 0329) and this changes the arithmetic's *shape*
  without intending to change its *result*; that is the exact scenario float
  non-associativity punishes. Land it alone, not batched with anything else.
- **vxn-2 engine couplings the extraction must not disturb** (verified
  2026-08-30). Four dest→source feedback paths with deliberate one-block
  latency: `stack_spread_mod` → next block's VoiceSpread source; smoothed
  `Lfo2Phase` applied as a wrapping delta into LFO2's phase; `lfo2.rate_mult`
  read from lane 0; LFO1-rate summed across stacks into next block's
  patch-global source. `scatter_stack_pitch` mutates `dest_vals` in place
  *between* `eval_dests` and the smoother's target capture. Patch-global dests
  are produced by a cross-stack lane-0 reduction the per-stack evaluator can't
  see. And `TargetFlags` gating is what keeps un-targeted paths bit-identical —
  that guarantee lives in the engine, not the evaluator, and stays there.
- **vxn-1b couplings**: bank.rs re-applies the evaluator's arithmetic piecewise
  for its Amp factoring (`shape`/`bend`/`slot_topology_gain` are `pub(crate)`
  for exactly this) — the shared crate must export those primitives or the Amp
  fast path loses bit-exactness. `Lfo1Rate` reads the *previous* block's total,
  a deliberate lag the eval-order contract preserves.
- If the generic form cannot be made bit-exact for both, **stop and re-scope** —
  [E049](../../epics/open/E049-shared-matrix-routing.md) takes no re-baselines.
  A macro stamping out a monomorphic evaluator per roster is the fallback: less
  elegant, same deduplication, no genericity tax.
