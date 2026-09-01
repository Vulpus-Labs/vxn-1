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

- [x] One evaluator in `vxn-core-matrix`, used by both synths; neither carries
      its own lane loop.
- [x] Every [0331](0331-matrix-golden-vector-harness.md) case passes through
      both the scalar and banked paths, bit-exactly.
- [x] Emitted asm for the banked evaluator contains NEON 4-wide arithmetic under
      both synths' lane counts. Grep the **mnemonic** for `.4s`, not the operands
      — see [[vxn1-neon-grep-pitfall]]; `grep 'v\d+\.4s'` returns nothing on
      genuinely vectorised ARM64 and would make success look like failure.
- [x] Null test against the pre-ticket render passes: difference peak
      ≤ −100 dBFS (see [E049](../../epics/closed/E049-shared-matrix-routing.md)
      §"The bar"). Re-capture the render hashes if they moved, and say so.
      *`-inf dBFS` on both — bit-identical, so no hash moved.*
- [x] Benchmarks: vxn-2's `matrix_eval_*` improve (it is gaining vectorisation);
      vxn-1b's hold. A vxn-1b regression means the generic form cost it
      something the monomorphic one didn't — measure and report before accepting.
      *Measured and reported below. The AC's premise had been overtaken: vxn-2
      gained its vectorisation in 0328, so its win here is the hoisted scale
      array (−33% on `matrix_eval_scaled`), not SIMD. vxn-1b's banked path holds
      to +1.6%; its scalar reference — which nothing but tests calls — is +16%,
      attributed below.*

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
  [E049](../../epics/closed/E049-shared-matrix-routing.md) takes no re-baselines.
  A macro stamping out a monomorphic evaluator per roster is the fallback: less
  elegant, same deduplication, no genericity tax.

## Close-out (2026-09-01)

- **One evaluator, in [eval.rs](../../crates/vxn-core-matrix/src/eval.rs).**
  `eval_dests` (scalar, per-voice, raw slots) and `eval_dests_bank` (dest-major,
  const-generic over `L`) are vxn-1b's bank form transcribed, generic over
  `MatrixRoster`. Neither synth carries a lane loop: `vxn1b_engine::eval`'s two
  entry points and `vxn2_engine::matrix::eval_dests` are now three-line bindings
  of roster and widths.
- **Both synths gained a `MatrixRoster` impl**, and it is generated, not written
  twice: a new `matrix_roster!` beside `matrix_enum!` forwards to the columns
  those enums already carry, plus a `const {}` guard that the source/dest enum is
  exactly the roster table plus one sentinel row (the `+ 1` every lookup rests
  on). `Vxn1bRoster` and `Vxn2Roster` are one declaration each.
  - The roster is read for the [storage](../../crates/vxn-core-matrix/src/storage.rs)
    width guards and nothing else — a compiled `Route` already carries the folded
    gain and the scale source's polarity. That is 0329's scheme finally having
    the consumer it was designed against.
- **The golden table covers the shipped code now.**
  `golden::eval_paths` registers `shared/scalar` and `shared/banked` alongside
  the harness's own pair, so all 36 cases, the per-lane sweep and the
  non-exact-value reassociation sweep run through the evaluator both synths ship,
  four-way bit-exact. `the_runner_has_more_than_one_path_to_compare` now names
  the two shipped paths, because `MIN_EVAL_PATHS` alone would be satisfied by the
  harness comparing itself with itself. Two small roster-driven endpoint
  adapters (`RosterSource` / `RosterDest`) are what let a case row become a
  `MatrixSlot`.
- **Null test `-inf dBFS` on both engines** — bit-identical renders, so neither
  render hash can have moved and no re-capture is needed. `cargo test
  --workspace` green: 91 test binaries, 0 failures. `vxn-asm-check` clean, every
  watched path above its floor.
- **4-wide NEON confirmed under both synths' instantiations.** Post-LTO
  `llvm-objdump` (Homebrew LLVM 22) on the linked `matrix` bench binaries, via
  `#[no_mangle]` `#[inline(never)]` probes removed before commit; counted on the
  **mnemonic** per [[vxn1-neon-grep-pitfall]]. vxn-1b `L = 8`: 579 instructions,
  84 `.4s`, 72 `.2s`. vxn-2 `L = 8`: 558 / 83 / 72. Both show the accumulate as
  `fmul.4s` → `ldr q` → `fadd.4s` → `str q`, and the curve arms carry
  `fabs.4s` / `fsqrt.4s` from `pol_abs` / `shape_log`.
  - Both synths use `L = 8`, so "both lane counts" is one number twice. A third
    probe at `L = 4` — the genericity's only other exercise in the repo — stays
    vectorised but narrows to `.2s` (61 of them, 1 `.4s`): at a trip count equal
    to the vector width the loop vectoriser stands down and SLP pairs it
    instead. Not a regression for anything that ships, and `.2s` is still
    vectorised (`vxn-asm-check`'s own rule), but worth knowing before someone
    reads a `.4s` count as the whole answer.

### Benchmarks

Measured by building the pre- and post-ticket bench binaries side by side and
running them **interleaved**, because absolute numbers drift several percent
between builds. Criterion's `--baseline` is unusable across these two: both
synths name their benches `matrix/matrix_eval_full`, so they share a
`target/criterion` directory and each run clobbers the other's saved baseline —
which is how a first pass produced a confident `-65%` on a case that had got
slower. Compare the printed numbers, not the change lines.

| bench | pre | post | Δ |
|---|---|---|---|
| vxn-2 `matrix_eval_scaled` | 108.4 ns | **72.9 ns** | **−32.7%** |
| vxn-2 `matrix_compile_full` | 79.86 ns | 76.67 ns | −4.0% |
| vxn-2 `matrix_eval_full` | ~96.6 ns | ~96.6 ns | flat |
| vxn-2 `matrix_eval_empty` | 21.67 ns | 22.03 ns | +1.6% |
| vxn-1b `matrix_bank_full` | 130.1 ns | 132.2 ns | +1.6% |
| vxn-1b `matrix_bank_scaled` | 103.1 ns | 104.8 ns | +1.6% |
| vxn-1b `matrix_eval_full` (scalar) | 27.88 ns | 32.46 ns | **+16.4%** |
| vxn-1b `matrix_eval_scaled` (scalar) | 41.9 ns | 46.3 ns | +10.5% |

- **vxn-2's big win is not vectorisation** — 0328 already gave it that. It is
  that the canon hoists the per-route `scale` array out of the route loop, while
  vxn-2's copy declared it inside and re-initialised eight floats per route.
  `matrix_eval_scaled`, whose every route is scaled, was paying that sixteen
  times per stack. `compile_full`'s −4% is `RouteList::compile` becoming a thin
  wrapper over the new `compile_slots`.
- **vxn-1b's banked path is +1.6%**, consistent across three interleaved rounds
  on both cases. The arithmetic is bit-identical (null test `-inf`) and the SIMD
  counts match, so this is instruction selection and code layout — the band
  `vxn-asm-check`'s own docs decline to police. Reported rather than accepted
  silently, per this ticket's AC.
- **vxn-1b's scalar path is +16%**, and the ticket asked for this to be measured
  before acceptance, so: isolated with a verbatim copy of the pre-ticket body
  compiled into the same bench binary, the gap splits in two. ~2.8 ns is the
  body being compiled anywhere other than the engine crate (the verbatim copy
  reads 30.8 ns against the original's 27.9); ~2.6 ns is the generic form against
  the monomorphic one. This path has **no production caller in either synth** —
  it is the reference implementation the bank is proved against, reached only by
  tests and this bench — so the cost is paid by the test suite, not the audio
  thread.
- **Two attempted recoveries, both rejected on measurement**, recorded so nobody
  retries them:
  - Binding `slot_gain` to a `let` before the shape (matching the isolated
    copy's statement order): **worse**, 32.5 → 41.8 ns.
  - Taking the slot table as a fixed-width `&[MatrixSlot; N]` so the loop keeps a
    compile-time trip count: **neutral**, and it forces the golden harness to pad
    every case. Reverted to the slice.
  - A third, on the banked side, is documented on `eval_dests_bank` itself:
    taking `&RouteList<N>` rather than `&[Route]` measured ~4% *faster* in a
    same-binary A/B and ~1% *slower* standalone across three interleaved rounds.
    The A/B was reading code layout. Slice kept — one fewer const parameter, and
    the faster of the two as shipped.

### One real bug found on the way

`RouteList::compile` read `slot.scale_src.is_bipolar()` **unconditionally**,
including for the unwired sentinel. Harmless for both synths, whose generated
enums answer `false` there — but `MatrixRoster`'s lookups are documented to panic
out of range, so any endpoint type forwarding `is_bipolar` to its roster (which a
roster-generic one must) would have panicked on every unscaled route. The
golden adapter hit it immediately. Fixed by reading the column only when there is
a scale source; `scale_bipolar` is unread when `scale` is `None`, so the change
is behaviour-preserving, and the adapter now forwards *without* a sentinel guard
on purpose, so the trap stays loud if it is ever reintroduced.

### Also landed

- `RouteList::compile` split into `compile_slots` (the drop predicate, the cook
  and the order — one statement, now reachable from a `Vec`-shaped caller) plus
  `RouteList::from_slots` (that, landing in the `N`-wide array). `compile` is two
  lines over `from_slots`.
- `slot_topology_gain` moved into the shared crate and `RouteList::compile` now
  *calls* it, closing a gap vxn-1b's doc had already claimed was closed: the
  product was spelled twice, once for the Amp factoring and once inline in
  `compile`. vxn-1b re-exports it `pub(crate)` for `bank`; `slot_scale` and
  `slot_gain` are no longer re-exported, having no caller outside the evaluator.

### Left open

- `vxn1b_engine::preset`'s unused `Polarity` import. Pre-existing, unrelated, and
  this ticket lands alone.
