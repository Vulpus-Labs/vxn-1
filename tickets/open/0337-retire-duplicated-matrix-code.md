---
id: "0337"
product: monorepo
title: "Retire the duplicated matrix code; docs, benchmarks and epic close-out"
priority: medium
created: 2026-08-30
epic: E049
depends: ["0330", "0331", "0332", "0333", "0334", "0335", "0336"]
---

## Summary

Close [E049](../../epics/open/E049-shared-matrix-routing.md): delete what the
extraction replaced, update the docs that describe the old shape, and record
what actually shipped.

## Acceptance criteria

- [ ] Neither synth defines its own slot type, curve axes, scale VCA, route
      compilation, evaluator or smoother bank. Grep both trees for the retired
      names and show the result in the close-out.
- [ ] [vxn-2 PARAMETERS.md](../../vxn-2/PARAMETERS.md) and
      [vxn-2 DEVELOPERS.html](../../vxn-2/DEVELOPERS.html) describe the shared
      engine where they currently describe vxn-2's own.
- [ ] vxn-1b's `PARAMETERS.md` regenerated (`gen_parameters_doc`), including the
      smoothing class per destination now that it is declared data — a table
      readers cannot get from the source otherwise.
- [ ] `vxn-core-matrix` module docs carry the surviving hot-loop lessons rather
      than leaving them in vxn-2's matrix.rs: hoist every per-route decision
      above the lane loop; `clamp_unit` not `f32::clamp`; the branch in
      `shape_log` beats `copysign`. All three are counter-intuitive and all
      three were measured.
- [ ] Both render-hash baselines byte-identical to their **pre-epic** values —
      not merely to the previous ticket's. Check against the commit before 0329.
- [ ] Benchmark table in the close-out: `matrix_eval_full` / `matrix_eval_scaled`
      for vxn-2 and the equivalent for vxn-1b, before the epic and after.
- [ ] [ADR 0003](../../adrs/0003-vxn-core-matrix.md) status → Accepted, with any
      decision that did not survive contact recorded there rather than only in a
      ticket close-out.

## Notes

- If the evaluator ([0334](0334-share-the-evaluator.md)) was re-scoped to the
  macro fallback, say so plainly in the ADR — a future reader comparing the ADR
  to the code should not have to reconstruct why they differ.
- The epic's stated bar is **no re-baselines**. If one was taken anyway, that is
  the single most important thing the close-out records, and the ADR's
  "no intended behaviour change" claim needs amending to match.
