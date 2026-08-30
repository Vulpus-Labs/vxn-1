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

- [ ] One smoother bank in `vxn-core-matrix`, driven by the declared class.
- [ ] vxn-2's `PitchSmoother` and vxn-1b's `mod_smoothing` tiers both retire
      into it.
- [ ] Time constants unchanged per dest — same tau, same quantum, same cascade
      depth as each synth used before.
- [ ] `snap_to` on voice start preserved in both.
- [ ] vxn-1b's non-envelope-Amp smoothing still behaves exactly as before, and a
      comment at the call site says why it is not a `Smoothing` class.
- [ ] Both render-hash baselines byte-identical. Smoother state is audible; a
      diff here is a real regression, not rounding. **Note the tension:** the
      two-pass split changes the order operations are issued in but not the
      arithmetic any single element sees — each element still computes
      `y += c*(x-y)` from the same inputs — so it should be bit-exact. Verify
      that rather than assuming it; if it isn't, the reordering is wrong
      somewhere.
- [ ] The two-pass restructure is measured on a **linked binary**, before and
      after, and the numbers go in the close-out.
- [ ] The zipper/discontinuity regression tests in both synths still pass.

## Notes

- Check the tick sites before assuming they match: vxn-2 ticks its pitch
  smoother at `PITCH_SMOOTH_QUANTUM` (16 samples) inside the render loop, and
  vxn-1b at its own `PITCH_QUANTUM`. If the constants differ, they stay
  per-synth — the *class* is shared, the *rate* is a synth's render-loop
  property.
- Out of scope: adding smoothing to dests that don't have it today. Behaviour
  preserving means preserving the absences too. New classifications are a
  follow-up with listening tests behind them.
