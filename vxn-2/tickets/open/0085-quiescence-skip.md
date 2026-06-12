---
id: "0085"
title: "Quiescence-skip per stack + state/coeff-ramp freeze"
priority: medium
created: 2026-06-12
epic: E007
depends: ["0084"]
---

## Summary

Sixth ticket of [E007](../../epics/open/E007-optional-per-voice-filter.md).
The render path ([0084](0084-per-stack-filter-render-path.md)) filters every
active stack every block. A released voice contributes zero once its filter has
rung out — but **silent ≠ quiescent**: a highly resonant ladder keeps ringing
after its input goes silent. Skip the upsample + ladder for a stack only when
the *filter itself* has settled, keying on state magnitude, not input level
(reusing VXN1's `silent-skip-filter-state` lesson and its high-resonance edge).

## Design

Per stack, per block, skip the upsample + ladder when **both**:

- the stack is idle / its amp envelope is at zero (input will be zero this
  block), **and**
- all four ladder stage states (`s[0..4]`, both L and R kernels) are below
  `eps`, where `eps` is chosen to cover the resonance ring — *not* a denormal
  floor. Validate `eps` at the self-oscillation boundary (resonance → 1).

A skipped stack contributes exact zero to `os_bus`, so omitting its
upsample/ladder/accumulate is exact, not approximate.

On skip:

- **Freeze** ladder state — do not clear, do not advance. Frozen state is
  already ~0, so re-entry on the next note is glitch-free.
- **Freeze** the cutoff/resonance coeff ramps too (don't let them free-run
  through a skip) so coefficients don't jump on re-trigger; amp-envelope attack
  masks any residual discontinuity.
- Re-arm the quiescent flag on **note-on**.

Cheapest detection: when a stack *is* filtered, track its output block max-abs;
if `< eps` and input was zero, mark quiescent → skip next block. The interp FIR
history self-flushes after tap-length zero-input samples (finite), so gating on
"input zero AND state < eps" already accounts for its settling.

The **shared decimator always runs** regardless of how many stacks were skipped,
to flush its own delay line.

## Acceptance criteria

- [ ] A resonant voice's release tail is preserved intact — no clipped ring when
  the amp envelope hits zero before the filter has settled (compare tail RMS /
  duration against the no-skip path at resonance = 0.95).
- [ ] A truly settled voice (low resonance, long after note-off) is skipped:
  measurable cost drop on a held chord with released tails versus 0084's
  always-filter path.
- [ ] Skipped→active re-entry on note-on is click-free (frozen state ≈ 0;
  coeff ramp resumes without a jump).
- [ ] `eps` validated at resonance → 1: a self-oscillating voice is **never**
  wrongly skipped while ringing.
- [ ] Output with skip enabled matches the always-filter path within tolerance
  for all factory patches (skip is an optimisation, not an audible change).

## Notes

This mirrors VXN1's silent fast path, which froze ladder/HPF state and coeff
ramps and relied on the amp envelope to mask staleness on attack — with the same
high-resonance caveat that a too-tight `eps` clicks. Reuse that tuning as the
starting point.
