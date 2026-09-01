---
id: E049
product: monorepo
title: "Shared matrix routing — one modulation engine under vxn-1b and vxn-2, rosters stay per-synth (behaviour-preserving; null-tested, not bit-frozen)"
status: closed
created: 2026-08-30
---

> **The behaviour-preserving epic.** Unlike
> [E041](E041-shared-fx-unification.md), which unifies genuinely different
> declick idioms and accepts flagged re-baselines (still in progress: 0231,
> 0232 open), this one has **no intended behaviour change at all**. But
> behaviour-preserving is not bit-frozen — see the bar below.

## The bar: null-tested, not bit-frozen

The render-hash baselines are a **tripwire, not the bar**. A hash is binary — it
cannot express a tolerance — and several tickets here legitimately reorder float
operations: transposing accumulators (0328), sharing an evaluator across two
lane counts (0334), splitting the smoother's fused cascade into two passes
(0335). Float addition is not associative, so those change bits without changing
what anyone hears. Demanding an unchanged hash would rule out the very
restructuring the epic exists to do. (The baseline's own header already concedes
the point: the hash "rounds differently across targets and OS releases", so it is
CI-only and dev machines skip it.)

**The bar is a null test.** Render the reference patch before and after; the
difference signal's peak must sit at or below **−100 dBFS** — beneath the 16-bit
noise floor and far beneath audibility, while leaving ample room for last-bit
reordering (a reassociated sum of ≤16 `f32` terms perturbs by ~1e-7 relative,
around −140 dBFS).

One known way the audio bar trips on an innocent change: **a pitch
destination**. A reordered sum can move a pitch total by an ULP, and frequency
error *integrates* — on a sustained tone with quasi-static modulation the
rounding bias is systematic, phase drifts linearly, and a 2 s render can show a
difference peak near −75 dBFS from a change nothing could hear. (Random-sign
rounding only random-walks to ~−107 dBFS; the systematic case is what misses
the bar.) So for matrix-arithmetic tickets the primary check is a null test on
the **dest-total streams at control rate** — ULP-scale tolerance, no integrator
between the change and the measurement — with the audio null test as the
end-to-end backstop. Keep reference renders short, and when the audio bar trips
while the dest totals are clean, that is the listening-check path working as
designed, not a failed ticket.

The harness does not exist yet — the repo has only the hash, and **only vxn-2's
hash at that: vxn-1b has no render-hash baseline today**. Building the harness,
the vxn-1b baseline and a vxn-1b matrix bench is part of
[0329](../../tickets/closed/0329-vxn-core-matrix-crate-skeleton.md); the
comparator sits in `vxn_core_dsp::test_util` alongside the other
render-comparison helpers, and it gates every later ticket's verification.

Workflow: if the hash doesn't move, nothing changed and you are done. If it
moves, run the null test. If the null test passes, re-capture the hash and say so
in the close-out. **A change that exceeds −100 dBFS stops** for a listening check
and an explicit decision — it does not get re-baselined quietly.

**Two things stay strictly bit-exact**, because there a difference is a bug
rather than a consequence:

- **Two evaluator paths in the same build** (scalar vs banked). They evaluate
  the same routes in the same order by construction; vxn-1b already states this
  as its contract, and [0331](../../tickets/closed/0331-matrix-golden-vector-harness.md)
  generalises it.
- **Pure-movement tickets** — [0330](../../tickets/closed/0330-share-curve-vocabulary.md)
  (code moves crates, no arithmetic changes) and
  [0332](../../tickets/closed/0332-roster-row-declares-everything.md) (constants
  are transcribed, not recomputed). If those move a bit, something was
  mistranscribed.

## Why

vxn-1b's mod matrix is a hand-port of vxn-2's, and says so in its own headers.
Two copies of one design have drifted in both directions: each has picked up
improvements the other lacks. Adding the `abs` polarity, the polarity/shape
split and the scale-VCA bend (commits `bbff167`, `868faef`) meant writing the
same ~200 lines twice, by hand, in one sitting — none of it specific to FM or
subtractive synthesis.

[ADR 0003](../../adrs/0003-vxn-core-matrix.md) sets the seam: the **roster**
(what can be routed) is per-synth; the **mechanism** (how a routing is
evaluated) is shared. It also settles where target smoothing goes — post-sum,
per-destination, declared in the roster row — and splits the test surface so
mechanism assertions stop baking roster constants into their expected numbers.

## What each synth gains

| | vxn-1b gains | vxn-2 gains |
|---|---|---|
| Tables | — (already generated) | generated roster tables; five hand-kept parallel lists retire |
| Layout | — (already dest-major SoA) | vectorised evaluator via 0328 |
| Compilation | — (already precompiles) | per-block `RouteList`; per-voice loop stops redoing patch-constant work |
| Coherence | the verdict surface, trivially `Ok` today, live the moment a global dest appears | — |
| Smoothing | tiers become declared rather than a list in another module | same |

## Vectorisation is a per-stage question, not an afterthought

Every ticket that touches a per-lane loop states where that stage stands on
vectorisation and backs it with a measurement. What is known today, all
post-LTO on a linked binary:

| Stage | Today | Note |
|---|---|---|
| Source fan-out | not measured | vxn-1b transposes to SoA; the transpose itself has a cost nobody has priced |
| Route accumulate | **scalar** | gather/scatter on lane-major accumulators — [0328](../../tickets/closed/0328-matrix-dest-major-lane-accumulators.md) |
| Scale VCA | 2-wide | walks a contiguous local, so it vectorises inside the same function the accumulate doesn't |
| Target smoothing | **4-wide already** | but ~46% available from splitting the fused cascade — [0335](../../tickets/closed/0335-declared-target-smoothing.md) |
| Target application | not measured | likely dominated by `exp2`/`powf` per lane; vectorising upstream may not move the needle |

**Measure post-LTO.** `cargo rustc --emit asm` on a lib crate here runs no loop
vectoriser ([[vxn-per-crate-asm-has-no-vectoriser]]). Two claims in this epic's
own tickets were wrong before that was caught. Run the canary first.

## Sequencing

Smallest blast radius first. The curve vocabulary and the roster declaration
have no layout dependency and land early; the evaluator waits on 0328 because
**it cannot be shared until both synths agree on memory layout**. 0328 itself
waits on 0329's harness: its acceptance bar is a null test, unmeasurable until
the harness exists — and the reference renders must be captured before 0328
reorders any arithmetic, so the pre-epic reference really is pre-epic.

```text
0338 mutex fix — independent, land first

0329 skeleton + harness ─┬─ 0330 curve vocab ──┬─ 0333 slot + RouteList ─┐
                         ├─ 0332 roster decl ──┤                         │
                         │        └─ 0336 coherence                      ├─ 0334 evaluator ── 0335 smoothing
                         ├─ 0331 golden-vector harness ──────────────────┤     (0335 also needs 0332)
                         └─ 0328 vxn-2 dest-major ───────────────────────┘

0337 close-out — after everything above
```

Once 0329 lands, four lanes can run concurrently: **0338** (independent
throughout — different files entirely); **0328** (vxn-2 engine only, no shared
crate); **0330 → 0332** (serial with each other — 0332 extends the
`matrix_enum!` that 0330 moves — parallel with everything else); **0331**
(shared crate tests only). The merge point is 0333, which needs both 0330 and
0332; the choke point is 0334, which needs everything except 0335/0336.

## Tickets

| # | Ticket | Depends |
|---|---|---|
| [0328](../../tickets/closed/0328-matrix-dest-major-lane-accumulators.md) | vxn-2 matrix eval doesn't vectorise: transpose to dest-major | 0329 (harness) |
| [0329](../../tickets/closed/0329-vxn-core-matrix-crate-skeleton.md) | `vxn-core-matrix` skeleton + `MatrixRoster` seam + **null-test harness** + vxn-1b baseline & bench | — |
| [0330](../../tickets/closed/0330-share-curve-vocabulary.md) | Share the polarity/shape/scale-VCA vocabulary | 0329 |
| [0331](../../tickets/closed/0331-matrix-golden-vector-harness.md) | Golden-vector test harness + synthetic roster | 0329 |
| [0332](../../tickets/closed/0332-roster-row-declares-everything.md) | Roster row declares gain, taper, tier, smoothing | 0329 |
| [0333](../../tickets/closed/0333-share-slot-and-route-compilation.md) | Share slot/table + `RouteList` precompilation | 0330, 0332 |
| [0334](../../tickets/closed/0334-share-the-evaluator.md) | Share the evaluator, const-generic over lanes | 0328, 0331, 0333 |
| [0335](../../tickets/closed/0335-declared-target-smoothing.md) | Declared per-destination smoothing bank | 0332, 0334 |
| [0336](../../tickets/closed/0336-coherence-in-the-shared-engine.md) | Coherence predicate + vxn-1b's UI surface | 0332 |
| [0337](../../tickets/closed/0337-retire-duplicated-matrix-code.md) | Retire the duplicates; docs + close-out | all |
| [0338](../../tickets/closed/0338-vxn1b-topology-ring-delete-the-mutex.md) | **Get vxn-1b's audio thread off the matrix mutex** | — |

> **[0338](../../tickets/closed/0338-vxn1b-topology-ring-delete-the-mutex.md) is
> independent and urgent.** It fixes a live real-time hazard — vxn-1b's audio
> thread takes a `std::sync::Mutex` on every topology edit — and depends on none
> of the extraction work. Land it first, or in parallel; do not queue it behind
> the crate skeleton.

## Out of scope

- **Wire and state encodings.** vxn-2 nibble-packs for blob compatibility;
  vxn-1b widens its record and bumps the version. ADR 0003 §"Alternatives"
  keeps these divergent on purpose.
- **Layer ownership.** vxn-1b's two independent matrices stay vxn-1b's problem;
  the engine evaluates one table.
- **CLAP surface.** 8-of-16 vs 16-of-16 automatable depths is per-synth config.
- **What a destination means.** Applying a dest total to a filter coefficient, a
  phase increment or a VCA stays in the synth.
- **vxn-3.** It has no modulation matrix. It inherits the engine by declaring a
  roster if it ever grows one; nothing here is built for it speculatively.

## Done when

- Both synths route through `vxn-core-matrix`; neither carries its own copy of
  the slot type, the curve axes, the scale VCA, route compilation, the
  evaluator or the smoother bank.
- Mechanism is tested once, in the shared crate, against a synthetic roster, in
  the declarative `routes + sources ⇒ expected` form.
- The end-to-end null test against the pre-epic reference passes at ≤ −100 dBFS
  on both synths (0337). The hashes match their last captured values, with
  every re-capture named in a ticket close-out — §"The bar" above deliberately
  does not require them byte-identical to pre-epic.

## Close-out (2026-09-01)

All eleven tickets closed. Both synths route through `vxn-core-matrix`; neither
carries its own slot type, curve axes, scale VCA, route compilation, evaluator or
smoother bank, and neither has a production lane loop left in its matrix module.

### The bar was met with room to spare

**Both synths render bit-identically to the pre-epic reference — `-inf dBFS`.**

The epic was built around the expectation that this would *not* happen: §"The
bar" argues at length that transposing accumulators, sharing an evaluator across
two lane counts and splitting a fused smoother cascade all reorder float
operations, that float addition is not associative, and that demanding an
unchanged hash would rule out the restructuring the epic exists to do. It
budgeted for hash moves, specified a −100 dBFS null test as the real bar, and set
out a workflow for re-capturing hashes with each move named in a close-out.

None of that was needed. No hash moved, no re-capture happened, no listening
check was called for, and no step came near the bar. The reasons, in the order
they mattered:

- **0328** transposed vxn-2's accumulators without touching accumulation *order*,
  so the reordering the epic feared never occurred.
- **0333** did change the multiply grouping (`shaped · depth · scale` →
  `shaped · (gain · scale)`), but the reference patch wires no scale sources, so
  the two spellings coincide on that render. Real change, invisible here — worth
  knowing that the reference does not cover it, and that the golden harness's
  reassociation sweep does.
- **0334** shared an evaluator across both rosters and stayed bit-exact by
  construction: same routes, same order, one association.
- **0335**'s reordering was measured and **rejected** before it shipped.

So ADR 0003's "no intended behaviour change" claim held exactly, and is recorded
that way on acceptance rather than amended.

The mechanism that made the end-to-end check trustworthy was set up in
[0329](../../tickets/closed/0329-vxn-core-matrix-crate-skeleton.md): both
reference renders were captured once, before any arithmetic moved, and never
re-captured. Every null test across nine tickets has therefore compared against
the pre-epic render rather than the previous ticket's, and the "per-ticket passes
do not compose" problem this epic worried about never arose.

### What each synth actually gained

The table in §"What each synth gains" was written as a prediction. Measured, per
active stack, pre-epic → now:

| | vxn-2 | vxn-1b |
|---|---|---|
| `matrix_eval_scaled` | 125.9 → **72.7 ns** (−42%) | — |
| `matrix_eval_full` | 113.5 → **96.2 ns** (−15%) | — |
| `matrix_eval_empty` | 26.5 → **21.1 ns** (−20%) | — |
| `matrix_bank_scaled` | — | 113.9 → **105.7 ns** (−7.2%) |
| `matrix_bank_full` | — | 137.1 → **134.2 ns** (−2.1%) |

Plus vxn-2's per-block/per-stack split: `RouteList::compile` (~77 ns) is now paid
once per block instead of that work sitting inside every stack's eval, up to
sixteen times. vxn-1b was the donor for most of the mechanism, so it had least to
gain and still improved on both banked cases; its scalar reference path regressed
17%, which has no production caller and is attributed in 0334's close-out.

vxn-1b also gained the coherence surface (trivially `Ok` today, live the moment a
global destination appears), and — outside the extraction — the fix in
[0338](../../tickets/closed/0338-vxn1b-topology-ring-delete-the-mutex.md) that
got its audio thread off a mutex.

### Where the epic's own analysis was wrong

Three of this epic's stated premises did not survive measurement. All three are
recorded on the code rather than only here, because the next person to read that
code is the one who needs them.

1. **0335's 46% two-pass cascade win does not exist.** The premise — fusing the
   two stages forces `zip2`/`uzp2` interleaving — is false on this toolchain:
   both shapes emit 138 instructions and 96 `.4s` ops with **zero** shuffles, and
   the split measures 7.3% *slower*. The fused loop ships.
2. **Vectorisation claims must be measured post-LTO**, which this epic already
   knew (§"Vectorisation is a per-stage question") and which caught two wrong
   claims in its own tickets before they shipped. It held for the rest.
3. **A same-binary A/B measures code layout as much as it measures the code.**
   Twice — 0334's `&RouteList` vs `&[Route]` and 0335's fused vs split — an
   in-binary comparison pointed the opposite way to a standalone one. Build one
   binary per implementation and interleave; that is now the house rule.

### Left open

- vxn-2's TOML preset format has no `active` column, so muting a route, saving
  and reloading brings it back on. Pre-existing, found during 0333; vxn-1b does
  persist the flag. Wants its own ticket.
- vxn-1b's web build shares `SharedParams` as a UI model but never drains the
  topology ring, so it sits permanently full with a resync owed. Inert, and
  documented in `shared.rs`; a real fix is a web-side drain (found during 0338).
- vxn-1b's smoother has no microbenchmark — its ticking is per-lane inside the
  render loop and does not factor out the way vxn-2's does.
