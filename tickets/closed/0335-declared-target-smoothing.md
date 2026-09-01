---
id: "0335"
product: monorepo
title: "Target smoothing: post-sum, per-destination, driven by the declared class"
priority: medium
created: 2026-08-30
epic: E049
depends: ["0332", "0334"]
---

## Summary

Consume the `Smoothing` column declared in
[0332](0332-roster-row-declares-everything.md): one shared smoother bank,
applied **after** matrix summing, with each destination's class deciding its
filter.

Settles the question [ADR 0003](../../adrs/0003-vxn-core-matrix.md) §3 was
written to answer. Smoothing is *not* uniform and *not* per-route:

- **Post-sum, not per-route.** The filters are linear, so filtering each route
  then summing is mathematically identical to summing then filtering — at N×
  the cost and N× the state for N slots sharing a dest.
- **Per-destination, not uniform.** The right time constant is a property of how
  click-prone the destination is. `delay-mix` never clicks; pitch stairsteps
  audibly at every control-block edge.

Today vxn-2 smooths 8 pitch dests via `PitchSmoother`; vxn-1b runs three tiers
from [mod_smoothing.rs](../../vxn-1b/crates/vxn1b-engine/src/mod_smoothing.rs).
Same design, two implementations, both keyed off lists that can drift from the
roster.

## Design

### Bank layout — the measured win is structural, not SIMD

`PitchSmoother::tick` **already vectorises 4-wide** post-LTO (`dup.4s` to
broadcast the coefficient, `fsub.4s` chains). Do not start from "add SIMD here";
that premise was measured wrong twice before it was caught, because per-crate
`--emit asm` runs no vectoriser at all ([[vxn-per-crate-asm-has-no-vectoriser]]).

The win is available anyway, and it is **46%** — 16.9 ns → 9.1 ns on a linked
binary. The current tick fuses both cascade stages in one loop body, so stage 2
reads the stage-1 value just written and the vectoriser has to interleave with
`zip2`/`uzp2` shuffles. Two flat passes — stage 1 across the whole span, then
stage 2 across it — remove the shuffles entirely:

```rust
for i in 0..n { s1[i] += c * (target[i] - s1[i]); }
for i in 0..n { st[i] += c * (s1[i]     - st[i]); }
```

For that to be a flat span rather than a gather, two things must hold. **One
coefficient per class**, which both synths already satisfy — vxn-1b's smoother
says so outright ("the coefficient is *not* a field: it belongs to the tier ...
rather than to the quantity"). And **contiguous storage**, which needs the
roster to order destinations so each smoothing class occupies an unbroken run of
rows. That collides with vxn-2's frozen dest discriminants, but only if wire id
and storage row are the same number — decoupling them is a compile-time lookup
applied once per route in `RouteList::compile`, never in a lane loop. If the
ordering turns out to be more disruptive than the 46% justifies, the fallback is
a per-class index table and a gather, which keeps the two-pass structure (where
the win actually comes from) and loses only the contiguity.

| Class | Filter | Ticked |
|---|---|---|
| `Block` | none — held for the control block | — |
| `Quantum` | one-pole | per render quantum |
| `QuantumCascade` | two cascaded one-poles | per render quantum |
| `PerSample` | one-pole | per frame |

State is `[class member][lane]`, sized from the roster, so a synth pays only for
the dests it declares.

**The cascade is load-bearing.** A single pole is C0 but C1-broken: at a saw or
pulse LFO step the output *value* is continuous while its *velocity* jumps
0→max, and that velocity step is the click. Both synths arrived at two poles
independently. Do not "simplify" it.

**Reset on voice start.** A stolen or restarted voice must not glide from the
previous note's modulation. vxn-2's `PitchSmoother::snap_to` is the existing
model; the shared bank exposes the same and the synth calls it on note-on.

**The Amp exception stays synth-side.** vxn-1b smooths only the non-envelope
part of its VCA coefficient — the envelope part is per-frame exact and smoothing
it would smear the attack. That factoring is a property of vxn-1b's VCA, not of
routing: `Amp` is declared `Block` and the bank does its own thing with the part
it chooses. This is a deliberate limit on the abstraction. Preserve it; do not
try to express it in the engine.

## Acceptance criteria

- [x] One smoother bank in `vxn-core-matrix`, driven by the declared class.
- [x] vxn-2's `PitchSmoother` and vxn-1b's `mod_smoothing` tiers both retire
      into it.
- [x] Time constants unchanged per dest — same tau, same quantum, same cascade
      depth as each synth used before.
- [x] `snap_to` on voice start preserved in both.
- [x] vxn-1b's non-envelope-Amp smoothing still behaves exactly as before, and a
      comment at the call site says why it is not a `Smoothing` class.
- [x] Null test against the pre-ticket render passes: difference peak ≤ −100
      dBFS. *`-inf dBFS` on both — bit-identical, no hash moved.*
- [x] The two-pass restructure is measured on a **linked binary**, before and
      after, and the numbers go in the close-out. *Measured, and **rejected**:
      it is a 7.3% regression, not a 46% win. See below.*
- [x] The zipper/discontinuity regression tests in both synths still pass.

## Notes

- Check the tick sites before assuming they match: vxn-2 ticks its pitch
  smoother at `PITCH_SMOOTH_QUANTUM` (16 samples) inside the render loop, and
  vxn-1b at its own `PITCH_QUANTUM`. If the constants differ, they stay
  per-synth — the *class* is shared, the *rate* is a synth's render-loop
  property.
- **Ordering trap**: vxn-2's `scatter_stack_pitch` writes stack-pitch
  contributions into `dest_vals` *after* `eval_dests` and *before* the smoother
  captures its targets. The shared bank must capture post-scatter, or E022's
  stack pitch bypasses smoothing.
- vxn-1b's counterpart to `snap_to` is `snap_all`, and it takes
  `bank::LaneTargets` today — the shared bank needs a synth-neutral snap
  signature; the smoother is not bank-independent as it stands.
- vxn-1b also smooths **Pan** on the slow tier — mod_smoothing.rs's own module
  doc omits it (written before 0260). Four smoothed quantities there, not
  three; the roster row for Pan declares `quantum`.
- Out of scope: adding smoothing to dests that don't have it today. Behaviour
  preserving means preserving the absences too. New classifications are a
  follow-up with listening tests behind them.

## Close-out (2026-09-01)

- **One bank layer in [smoothing.rs](../../crates/vxn-core-matrix/src/smoothing.rs).**
  `CascadeBank<NR, L>` (two poles) and `OnePoleBank<NR, L>` (one), plus
  `class_count` / `class_rows` / `row_of` — the `const fn`s that turn a roster's
  declared `Smoothing` column into a bank's row set. Both synths' filters,
  state, snaps and settle predicates are now these; neither writes a recurrence.
- **vxn-2**: `PitchSmoother` is a **type alias** for
  `CascadeBank<N_PITCH_DESTS, STACK_LANES>` — not a wrapper, no state or
  arithmetic of its own. What stays in `matrix.rs` is the binding: `pitch_smoother`
  (cooks the coefficient at the *tick* rate, `sample_rate / PITCH_SMOOTH_QUANTUM`),
  `pitch_targets` (the row gather, spelled once so tick / snap / converged cannot
  gather differently), and `pitch_smoother_row`. Its three hand-rolled `const`
  blocks — `N_PITCH_DESTS`, `PITCH_DEST_ROWS`, `pitch_smoother_row` — became
  calls to the shared `const fn`s, so vxn-1b derives its banks the same way.
- **vxn-1b**: `MotionSmoother` keeps its whole public API — `bank.rs` is
  unchanged apart from one comment — and its four smoothed quantities became
  three shared banks: a `CascadeBank<2, 8>` for `Pitch`/`XModSweep`, an
  `OnePoleBank<4, 8>` for the two PWM poles + cross-mod + pan (they share a
  coefficient, so they share a bank), and an `OnePoleBank<1, 8>` for the Amp
  exception. `LaneOnePole` and the `one_pole_api!` macro that generated twelve
  delegating methods are both gone.
  - Its cascade rows are now **derived from the column** rather than
    `const PITCH = 0; const SWEEP = 1;`, and a `const {}` assert holds the
    per-quantum bank to the `quantum` column — declaring a new smoothed
    destination is a build error here instead of a silent stairstep. The assert
    knows about the deliberate 3-dests-to-2-poles PWM fold and says so.
- **Bit-identical on both synths**: null test `-inf dBFS`, so no hash moved and
  no tau changed. `cargo test --workspace` green (91 binaries, 0 failures),
  including both synths' zipper/discontinuity tests. `vxn-asm-check` clean.

### The two-pass restructure: measured, and not shipped

**The ticket's central optimisation does not reproduce.** Its premise was that
fusing the two cascade stages in one loop body forces the vectoriser to
interleave with `zip2`/`uzp2` shuffles, and that splitting into two flat passes
buys 46% (16.9 → 9.1 ns). Post-LTO `llvm-objdump` on a linked bench binary, both
shapes at `NR = 8`, `L = 8`:

| shape | instructions | `.4s` | `zip`/`uzp`/`trn` |
|---|---|---|---|
| fused (shipped) | 138 | 96 | **0** |
| two-pass split | 138 | 96 | **0** |

There were no shuffles to remove. The fused loop already vectorises cleanly, the
two shapes emit the same instruction count and the same SIMD, and the split is
**slower** — it only inserts a serialisation point between the stages. So the
fused loop ships, with the measurement recorded on it so nobody re-splits it.

New benches carry the numbers, because there was nothing to measure against:
`matrix_smoother_tick` and `matrix_smoother_converged` in
[vxn2-osc-bench](../../vxn-2/crates/vxn2-osc-bench/benches/matrix.rs).

| case | pre | post | Δ |
|---|---|---|---|
| `matrix_smoother_tick`, own struct → shared bank (both fused) | 7.01 ns | 7.01 ns | **0.0%** |
| `matrix_smoother_tick`, fused → two-pass split | 7.01 ns | 7.52 ns | **+7.3%** |
| `matrix_smoother_converged` | 20.57 ns | 20.58 ns | flat |
| vxn-1b `route_profile routed` (whole render) | 52.0× RT | 52.0× RT | flat |

The first row is the one that matters: **the extraction itself is exactly free**,
including the `pick` closure the row gather now goes through. That was isolated
deliberately — a third binary with the shared bank but the *old* fused loop reads
7.01 ns, identical to the pre-ticket struct, which is what separates "the shared
bank costs nothing" from "the split costs 7%".

**Measure standalone, one implementation per binary.** The same fused-vs-split
comparison *inside* one binary reported the split ahead; that was code layout.
This is the second time in E049 an in-binary A/B has pointed the wrong way (0334
hit it on `&RouteList` vs `&[Route]`), which is now a rule rather than an
anecdote.

### Two things kept per-synth on purpose

- **The tick schedule.** vxn-2 advances a whole stack every quantum
  (`tick_rows`); vxn-1b advances **only lanes with a live route**
  (`tick_lane`), and the same branch decides whether to re-cook that lane's
  oscillator increment, pulse width, PM index or pan gains — tick and cook are
  one test. Flattening vxn-1b to a bank-wide tick would advance lanes that
  currently freeze, and on a pitch destination an ULP-scale difference
  integrates into phase drift (E049 §"The bar"). Both shapes are on the shared
  bank and a test asserts they agree bit-exactly, so this is a schedule
  difference and not a second filter.
- **The Amp exception**, exactly as ADR 0003 §3 reserves it. `Amp` declares
  `block`; vxn-1b filters only the *static* part of its VCA coefficient at the
  frame rate, because smoothing the envelope part would smear the attack. The
  call site in `bank.rs` now says so in place.

### One doc correction worth naming

`lane_active`'s two clauses are not "still moving": a lane parked exactly on a
**nonzero** target reports active, because the second clause is `|state| > eps`.
That is load-bearing — it is what keeps a lane ticking after its route turns off
so it glides back to zero instead of snapping, and snapping is the click the
smoother exists to prevent. A test I wrote asserting the intuitive reading failed
against the shipped behaviour; the behaviour was right and the doc now states it.

### Left open

- vxn-1b's smoother has no microbenchmark of its own — its ticking is per-lane
  inside the render loop and does not factor out the way vxn-2's does. The
  whole-render `route_profile` figure above is the coverage it has, alongside the
  bit-identical null test.
- Out of scope and untouched, per the ticket: adding smoothing to destinations
  that lack it today. Behaviour-preserving means preserving the absences.
