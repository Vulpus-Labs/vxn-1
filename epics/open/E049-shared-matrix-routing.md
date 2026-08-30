---
id: E049
product: monorepo
title: "Shared matrix routing — one modulation engine under vxn-1b and vxn-2, rosters stay per-synth (behaviour-preserving; null-tested, not bit-frozen)"
status: open
created: 2026-08-30
---

> **The behaviour-preserving epic.** Unlike
> [E041](E041-shared-fx-unification.md), which unified genuinely different
> declick idioms and accepted flagged re-baselines, this one has **no intended
> behaviour change at all**. But behaviour-preserving is not bit-frozen — see
> the bar below.

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

The harness does not exist yet — the repo has only the hash. Building it is part
of [0329](../../tickets/open/0329-vxn-core-matrix-crate-skeleton.md), sited in
`vxn_core_dsp::test_util` alongside the other render-comparison helpers, and it
gates every later ticket's verification.

Workflow: if the hash doesn't move, nothing changed and you are done. If it
moves, run the null test. If the null test passes, re-capture the hash and say so
in the close-out. **A change that exceeds −100 dBFS stops** for a listening check
and an explicit decision — it does not get re-baselined quietly.

**Two things stay strictly bit-exact**, because there a difference is a bug
rather than a consequence:

- **Two evaluator paths in the same build** (scalar vs banked). They evaluate
  the same routes in the same order by construction; vxn-1b already states this
  as its contract, and [0331](../../tickets/open/0331-matrix-golden-vector-harness.md)
  generalises it.
- **Pure-movement tickets** — [0330](../../tickets/open/0330-share-curve-vocabulary.md)
  (code moves crates, no arithmetic changes) and
  [0332](../../tickets/open/0332-roster-row-declares-everything.md) (constants
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
| Route accumulate | **scalar** | gather/scatter on lane-major accumulators — [0328](../../tickets/open/0328-matrix-dest-major-lane-accumulators.md) |
| Scale VCA | 2-wide | walks a contiguous local, so it vectorises inside the same function the accumulate doesn't |
| Target smoothing | **4-wide already** | but ~46% available from splitting the fused cascade — [0335](../../tickets/open/0335-declared-target-smoothing.md) |
| Target application | not measured | likely dominated by `exp2`/`powf` per lane; vectorising upstream may not move the needle |

**Measure post-LTO.** `cargo rustc --emit asm` on a lib crate here runs no loop
vectoriser ([[vxn-per-crate-asm-has-no-vectoriser]]). Two claims in this epic's
own tickets were wrong before that was caught. Run the canary first.

## Sequencing

Smallest blast radius first. The curve vocabulary and the roster declaration
have no layout dependency and land early; the evaluator waits on 0328 because
**it cannot be shared until both synths agree on memory layout**.

```
0328 (vxn-2 dest-major)  ─────────────────┐
                                          ▼
0329 crate skeleton ── 0330 curve vocab ── 0333 slot + RouteList ── 0334 evaluator
       │                     │                                          │
       │                     └── 0331 golden-vector harness ────────────┤
       │                                                                │
       └── 0332 roster declaration ── 0335 smoothing ── 0336 coherence ──┴── 0337 close-out
```

## Tickets

| # | Ticket | Depends |
|---|---|---|
| [0328](../../tickets/open/0328-matrix-dest-major-lane-accumulators.md) | vxn-2 matrix eval doesn't vectorise: transpose to dest-major | — |
| [0329](../../tickets/open/0329-vxn-core-matrix-crate-skeleton.md) | `vxn-core-matrix` skeleton + `MatrixRoster` seam + **null-test harness** | — |
| [0330](../../tickets/open/0330-share-curve-vocabulary.md) | Share the polarity/shape/scale-VCA vocabulary | 0329 |
| [0331](../../tickets/open/0331-matrix-golden-vector-harness.md) | Golden-vector test harness + synthetic roster | 0329 |
| [0332](../../tickets/open/0332-roster-row-declares-everything.md) | Roster row declares gain, taper, tier, smoothing | 0329 |
| [0333](../../tickets/open/0333-share-slot-and-route-compilation.md) | Share slot/table + `RouteList` precompilation | 0330, 0332 |
| [0334](../../tickets/open/0334-share-the-evaluator.md) | Share the evaluator, const-generic over lanes | 0328, 0331, 0333 |
| [0335](../../tickets/open/0335-declared-target-smoothing.md) | Declared per-destination smoothing bank | 0332, 0334 |
| [0336](../../tickets/open/0336-coherence-in-the-shared-engine.md) | Coherence predicate + vxn-1b's UI surface | 0332 |
| [0337](../../tickets/open/0337-retire-duplicated-matrix-code.md) | Retire the duplicates; docs + close-out | all |
| [0338](../../tickets/open/0338-vxn1b-topology-ring-delete-the-mutex.md) | **Get vxn-1b's audio thread off the matrix mutex** | — |

> **[0338](../../tickets/open/0338-vxn1b-topology-ring-delete-the-mutex.md) is
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
- Both render-hash baselines are byte-identical to their pre-epic values.
