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

Close [E049](../../epics/closed/E049-shared-matrix-routing.md): delete what the
extraction replaced, update the docs that describe the old shape, and record
what actually shipped.

## Acceptance criteria

- [x] Neither synth defines its own slot type, curve axes, scale VCA, route
      compilation, evaluator or smoother bank. Grep both trees for the retired
      names and show the result in the close-out.
- [x] [vxn-2 PARAMETERS.md](../../vxn-2/PARAMETERS.md) and
      [vxn-2 DEVELOPERS.html](../../vxn-2/DEVELOPERS.html) describe the shared
      engine where they currently describe vxn-2's own.
- [x] vxn-1b's `PARAMETERS.md` regenerated (`gen_parameters_doc`), including the
      smoothing class per destination now that it is declared data — a table
      readers cannot get from the source otherwise.
- [x] `vxn-core-matrix` module docs carry the surviving hot-loop lessons rather
      than leaving them in vxn-2's matrix.rs: hoist every per-route decision
      above the lane loop; `clamp_unit` not `f32::clamp`; the branch in
      `shape_log` beats `copysign`. All three are counter-intuitive and all
      three were measured.
- [x] Null test against the **pre-epic** render — not merely the previous
      ticket's — passes at ≤ −100 dBFS. Check against the commit before 0329;
      per-ticket passes do not compose into an end-to-end one, and a slow drift
      across eight tickets is exactly what this catches.
- [x] Benchmark table in the close-out: `matrix_eval_full` / `matrix_eval_scaled`
      for vxn-2 and the equivalent for vxn-1b, before the epic and after.
- [x] [ADR 0003](../../adrs/0003-vxn-core-matrix.md) status → Accepted, with any
      decision that did not survive contact recorded there rather than only in a
      ticket close-out.

## Notes

- If the evaluator ([0334](0334-share-the-evaluator.md)) was re-scoped to the
  macro fallback, say so plainly in the ADR — a future reader comparing the ADR
  to the code should not have to reconstruct why they differ.
- The epic's bar is a **null test at −100 dBFS**, not an unchanged hash. Record
  the measured difference peak in the close-out, not just "passed" — the number
  is what a future reader needs to judge whether a later change is in the same
  league. If any ticket exceeded the bar and was accepted after a listening
  check, that is the single most important thing the close-out records, and the
  ADR's "no intended behaviour change" claim needs amending to match.

## Close-out (2026-09-01)

### Nothing duplicated survives

Grepping both synth trees for a **definition** of each retired name:

| Retired name | vxn-1b | vxn-2 |
|---|---|---|
| `struct MatrixSlot` / `MatrixTable` | — | — |
| `struct Route` / `RouteList` | — | — |
| `enum Polarity` / `enum Shape` | — | — |
| `fn scale_norm` / `pol_*` / `shape_*` / `clamp_unit` | — | — |
| `struct PitchSmoother` | n/a | — (now a type alias) |
| `struct LaneOnePole`, `one_pole_api!` | — | n/a |

Every hit is a re-export, a type alias or a binding. The one apparent survivor,
`vxn1b_engine::eval::eval_dests_bank<const L>`, is a three-line forward to the
shared evaluator; it keeps the name because `bank.rs` and the bench call it.

**No production lane loop remains in either synth.** Every
`for k in 0..STACK_LANES` left in `vxn2-engine/src/matrix.rs` is inside `#[cfg(test)]`
— eleven of them, all in tests. Module sizes, pre-epic → now:

| | pre-epic | now | Δ |
|---|---|---|---|
| `vxn2-engine/src/matrix.rs` | 2326 | 1669 | −657 |
| `vxn1b-engine/src/eval.rs` | 1009 | 686 | −323 |
| `vxn1b-engine/src/mod_smoothing.rs` | 521 | 517 | −4 |

`mod_smoothing.rs` barely moved because what left (the recurrences, the state,
`LaneOnePole`, the twelve-method macro) was replaced by roughly as much prose
about the class derivation and the per-lane gating. The line count is the wrong
measure there; the right one is that it no longer contains a filter.

### The null test, end-to-end against the pre-epic render

**`-inf dBFS` on both synths — bit-identical, not merely inside the bar.**

This is stronger than "each ticket passed", and the reason is worth recording:
both `reference_render.f32` files were captured once, at `3d14b0a` (0329), and
**never re-captured** — `git log` on each shows a single commit. Every null test
run across all nine tickets has therefore been against the pre-epic render
already, and today's still reports zero difference. Both render hashes are
likewise untouched.

The epic budgeted for hash moves from reassociation and got none. 0328 reordered
accumulators, 0333 changed the multiply grouping and 0334 shared an evaluator
across two lane counts; each landed on the same bits. No listening check was ever
needed, and no step exceeded the bar — so ADR 0003's "no intended behaviour
change" claim needed no amendment, and says so on acceptance.

### Benchmarks, pre-epic → post-epic

Both binaries built from a worktree at the pre-epic commit and run **interleaved**
against today's, two rounds each. vxn-2's pre-epic point is `7ddc451` (the last
commit before 0329's code); vxn-1b's is `3d14b0a`, because vxn-1b had no matrix
bench until 0329 added one — that commit predates every arithmetic change, so it
is pre-epic for this purpose. vxn-1b's bench file is byte-identical between the
two points, so its four cases compare directly.

**vxn-2** (per active stack):

| case | pre-epic | now | Δ |
|---|---|---|---|
| `matrix_eval_full` | 113.5 ns | 96.2 ns | **−15.2%** |
| `matrix_eval_scaled` | 125.9 ns | 72.7 ns | **−42.3%** |
| `matrix_eval_empty` | 26.5 ns | 21.1 ns | **−20.4%** |

Read those with one caveat: 0333 moved `RouteList::compile` *outside* the timed
loop, because it is now paid once per block rather than once per stack. So the
post column excludes ~77 ns (`matrix_compile_full`) of work the pre column paid
inside every stack's eval — up to sixteen times a block. The comparison is
per-stack-half against per-stack-half, which is the half the epic improved; the
work that left it did not vanish, it moved to a rate that pays for itself from
the second voice on.

`matrix_eval_scaled` is the headline, and it is two wins stacked: 0328's
dest-major layout (the accumulate went 4-wide) and 0335's hoisting of the
per-route scale array out of the route loop, which vxn-2's own copy re-initialised
sixteen times per stack.

**vxn-1b** (banked = what the render loop runs; scalar = the reference the bank
is proved against):

| case | pre-epic | now | Δ |
|---|---|---|---|
| `matrix_bank_full` | 137.1 ns | 134.2 ns | **−2.1%** |
| `matrix_bank_scaled` | 113.9 ns | 105.7 ns | **−7.2%** |
| `matrix_eval_full` (scalar) | 28.06 ns | 32.9 ns | +17.3% |
| `matrix_eval_scaled` (scalar) | 41.94 ns | 46.7 ns | +11.3% |

vxn-1b was the donor for most of the mechanism, so it had less to gain, and both
its banked cases still improved. The scalar reference regressed and 0334 has the
attribution: roughly half is the body being compiled outside the engine crate,
half the generic form against the monomorphic one. **It has no production caller
in either synth** — it is the reference implementation the bank is held bit-exact
against, reached only by tests and the bench — so the cost lands on the test
suite, not the audio thread. `route_profile routed` (whole render) is unchanged
at 52.0× realtime.

### Docs

- **[vxn-2 DEVELOPERS.md](../../vxn-2/DEVELOPERS.md) / [.html](../../vxn-2/DEVELOPERS.html)**
  §3.2 now opens with the seam and a table of what moved to `vxn-core-matrix`,
  says plainly what stays VXN2's (the roster, the packed-`u32` wire encoding,
  which 8 of 16 depths automate, what a destination *means*), and describes
  evaluation as the two rates it now has rather than as one per-block fan-out.
  Both files edited in step.
- **[vxn-2 PARAMETERS.md](../../vxn-2/PARAMETERS.md)** gains the same framing on
  the Mod matrix section, and the slot table gains the **`enabled`** row — 0333
  gave vxn-2 the on/off switch and the doc had never mentioned it.
- **[vxn-1b PARAMETERS.md](../../vxn-1b/PARAMETERS.md)** regenerated, with the
  destination table widened from `Label | Wire name` to **`Label | Wire name |
  Gain | Taper | Smoothing`**. That is the table the ticket asked for: the
  smoothing class is declared data now, and a reader cannot get it from the
  source without reading two modules. The generator emits the Amp exception and
  the `Cutoff`/`HpfCutoff` `block` reading underneath, because both look like
  omissions and neither is.
- **The hot-loop lessons already moved** with the code they constrain — the
  `clamp_unit`-over-`f32::clamp` measurement sits on `clamp_unit`, the
  `shape_log` branch-over-`copysign` measurement on `shape_log`, the hoisting
  discipline in `eval`'s module header. What this ticket fixed is the *other*
  half: vxn-2's `matrix.rs` still carried an "Inner-loop shape" section citing
  `253 ns → 133 ns` for a hoist that now lives elsewhere, flagged as a follow-up
  in 0328's close-out. It now points at the shared crate and keeps only the two
  facts that are VXN2's — the transposed buffers (a *precondition* of sharing,
  not an optimisation on top) and the per-stack-vs-per-block rate split.

### ADR 0003 → Accepted

With a new **"What shipped"** section recording the three things that did not
survive contact, so a reader comparing the ADR to the code does not have to
reconstruct why they differ: the extraction landed whole rather than as the macro
fallback; §3's two-pass cascade optimisation was measured and rejected; and §2
gained `matrix_roster!` as a companion generator. The "no intended behaviour
change" claim is recorded as having held exactly.

### Verified

`cargo test --workspace` green — 91 test binaries, 0 failures. `vxn-asm-check`
clean, every watched path above its floor.
