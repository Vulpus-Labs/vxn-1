---
id: "0048"
product: vxn-3
title: "vxn-3 pattern engine: polymeter, probability, retrig n-over-m"
priority: high
created: 2026-06-15
epic: E021
depends: ["0047"]
---

## Summary

Turn the basic step grid (0047) into the actual differentiator: independent
per-track lanes that phase (polymeter), per-trig probability, and retrig
n-over-m. This is the "accessible interesting rhythm" thesis — cheap sequencer
logic, no new DSP.

## Design

- **Polymeter.** A pattern is N independent track lanes, each with its own
  length and clock divisor — not one shared grid. Each lane has a **lane-local
  tick base** (e.g. lane A triplets, lane B semiquavers); the sequencer
  resolves each lane against the host clock independently so lanes phase
  (len 16 vs 12 vs 7 → drift).
- **Per-trig probability.** Each trig carries a probability; on each pass the
  trig fires or is skipped. (Conditional/condition-group trigs are deferred.)
- **Retrig n-over-m (patches model).** A trig owns a sub-window of `m` steps
  and fires `n` times within it. Per-trig params: count `n`, span `m`, timing
  curve (even / accel / decel), velocity ramp. Retrig hits are sample-accurate
  on the lane-local grid.
- **Trig data model.** Distinguish trig *attributes* (probability, retrig n/m,
  velocity, micro-timing) which live on the trig, from continuous params (those
  get p-locked in 0050). Keep this split explicit (ADR 0001 §3a).

## Acceptance criteria

- [ ] Tracks with different lengths/divisors phase against each other audibly
      over a loop.
- [ ] Per-trig probability thins triggers statistically; probability 1.0 always
      fires, 0.0 never.
- [ ] Retrig produces `n` hits across `m` steps with the selected timing curve
      and velocity ramp, sample-accurate to the lane grid.
- [ ] All scheduling stays sample-accurate to the host clock across block
      boundaries and transport jumps.
- [ ] Process callback remains allocation-free.

## Notes

- Lane-local tick base is the same time unit p-lock `N` uses (0050) — keep them
  consistent.
- Out of scope: conditional trig groups (fill/neighbour), micro-timing UI,
  p-locks (0050).
- Design: `vxn-3/adrs/0001` §2.
