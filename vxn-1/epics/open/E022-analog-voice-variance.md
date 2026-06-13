---
id: E022
title: Analog per-voice variance
status: open
created: 2026-06-13
---

## Goal

Deepen the "real polysynth" character by extending per-voice
variation beyond the single mechanism shipped today (a slow
bounded random walk on oscillator pitch, `drift_value`). Real
analog poly hardware has two distinct sources of per-voice
spread:

1. **Slow thermal drift** — a walk. Pitch and VCF cutoff both
   ride exponential converters, so they wander; and because the
   VCF usually tracks the keyboard CV, its cutoff inherits a
   *scaled copy* of pitch drift through the tracking path.
2. **Fixed per-voice tolerance** — a constant offset set at
   power-on, from component tolerance (caps, resistors, expo
   trims). Affects envelope times, sustain, base cutoff and
   resonance. Not a walk; a frozen per-lane offset.

vxn-1 currently models only (1), and only on pitch. This epic
adds the missing couplings and the fixed-tolerance layer.

## Tickets

- 0123 — keytrack tracks drifted pitch, not the raw key value
  (the tracking-path coupling from mechanism 1).
- 0124 — fixed per-voice variance on env times, sustain, base
  cutoff and resonance (mechanism 2).

## Constraints

- **Equivalence/baseline discipline.** The layer-sum
  equivalence tests assume two layers sum to exactly twice one,
  which holds only when per-voice decorrelation is zeroed
  (today via `drift_amount = 0`). Every new variance source
  must short-circuit to bit-exact-shared when its amount is 0,
  or the tests must explicitly zero it.
- **Self-resonance whistle stays in tune.** Base-cutoff
  variance must be small enough that two self-oscillating
  voices beat gently, never sound detuned (see 0124).
- **Determinism.** All seeds derive from the existing per-lane
  seeding scheme so renders stay reproducible for `baseline.rs`.
