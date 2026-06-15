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

## Close-out (2026-06-15)

- **Polymeter.** Each track's lane carries its own `len` *and* lane-local tick
  `step_beats` ([sequencer.rs](../../vxn-3/crates/vxn3-engine/src/sequencer.rs));
  [lane.rs](../../vxn-3/crates/vxn3-engine/src/lane.rs) resolves each lane
  against the host clock independently. Test
  `pattern::tracks_of_different_lengths_phase` — len 16 vs 12 fire at distinct
  periods (16·step vs 12·step) so they drift apart after coinciding.
- **Per-trig probability.** `Step::probability`, drawn **once per primary trig**
  via a per-track xorshift32 (so a step straddling a block boundary can't be
  re-rolled). Tests `probability_extremes_are_deterministic` (p=1 → 256/256,
  p=0 → 0) and `probability_half_thins_statistically` (p=0.5 → ~half over 1024).
- **Retrig n-over-m.** `Retrig { n, m, curve, vel_end }`; a fired retrig anchors
  a window of `m` lane steps and emits `n` hits along the
  `RetrigCurve` (Even/Accel/Decel), carried in-flight across blocks. Tests
  `retrig_even_is_sample_accurate_across_blocks` (4 hits at 0/3000/6000/9000
  frames spanning 2 steps + multiple blocks), `retrig_velocity_ramps_linearly`
  (1.0→0.75→0.5→0.25), `retrig_accel_gaps_shrink` (strictly shrinking gaps).
- **Trig-attribute vs continuous split.** Probability + retrig live on the
  `Step` (no base to revert to); continuous params are untouched here and get
  p-locked in 0050 — the ADR §3a split is kept explicit in the data model.
- **Sample-accurate across blocks + jumps.** All hit frames are computed from
  absolute beat positions; `lane.rs` detects a transport discontinuity (>½ step
  off the expected continuation) and resyncs, dropping in-flight retrig. Test
  `transport_jump_resyncs_the_lane` (jump to the next bar re-fires the step at
  the jumped block's start). Cross-block accuracy also covered by the retrig
  tests (block size 512, windows spanning several blocks).
- **Allocation-free.** Scheduler emits into a pre-allocated, capacity-guarded
  `Vec<Hit>` (drops beyond cap, never reallocates) and sorts in place. Test
  `process_block_alloc_free_with_probability_and_retrig` — 0 allocs over ~300
  blocks with probability + retrig across 8 polymetric tracks.
- vxn3 tests: dsp 6, engine 6 + groove 4 + pattern 8, clap 3 + smoke 3 — all
  pass; vxn3 crates clippy-clean; `clap-validator validate` 0 failures.
