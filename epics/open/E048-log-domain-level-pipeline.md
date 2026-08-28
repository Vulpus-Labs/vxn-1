---
id: E048
product: vxn-2
title: "One log-domain level accumulator for the operator amplitude chain"
status: open
created: 2026-08-29
---

> Every contributor to an operator's amplitude combines by **addition in level
> units** (~0.75 dB, at the hardware's 1/32 resolution), with the hardware's two
> clamps in the hardware's order and a **single** conversion to linear at the
> end. Retires `ks_level_mult` and `vel_factor` as independent multipliers.
> Design and rationale: [ADR 0010](../../vxn-2/adrs/0010-log-domain-level-pipeline.md).

## Why

Three shipped calibration bugs were all the same shape — an amplitude
contributor that was never in the domain the calibration is expressed in:

- key level scaling faded a linear multiplier to the keyboard edge, producing
  −0.1 dB where hardware produces −49 dB (a control that did nothing);
- velocity ceilings at unity because a multiplier does, where the hardware's
  signed level offset reaches **+5.25 dB** at `kvs 7` — the missing "ting" on
  every tine patch, and the reason `Electric Boogaloo` reads dull;
- the ceiling was `.min(1.0)` on a linear product, matching `min(127)` only by
  the coincidence that `level_to_amp(99) = 1.0`.

The pitch chain has always summed its contributors in semitones and has never
had a calibration bug. This epic gives amplitude the same treatment.

Note what this does **not** do: the feedback ladder was wrong by one rung on a
correctly-logarithmic scale. A shared domain makes calibration errors visible
and expressible; each table still has to be checked against the reference.

## Prerequisites

The key-scaling port and feedback ladder land first — 0323 consumes
`ks_level_offset` directly.

## Planned tickets

Chain: **0323 → 0324 → 0325 → 0326**, with **0327** independent.

- [ ] **0323** — Level accumulator: `scaleoutlevel` table, `i32` units at 1/32
      resolution, hardware clamp order, one `exp2` at the end. `ks_level_offset`
      feeds it; `ks_level_mult` retired.
- [ ] **0324** — `ScaleVelocity` port (64-entry table, signed offset, boosts
      above nominal). `vel_factor` retired.
- [ ] **0325** — Renormalise full scale to the maximum attainable level so the
      velocity boost has headroom without disturbing stage 8's `[0,1]` invariant.
- [ ] **0326** — Master-volume re-sweep of the factory bank; one audition pass
      covering key scaling, feedback and velocity together.
- [ ] **0327** — Fold ratio and detune onto the semitone accumulator in
      `compute_base_hz`. Consistency, not correction.

## Acceptance

- Every amplitude contributor enters as a signed level offset; no contributor
  is applied as an independent linear multiplier.
- Ported tables match the reference exactly on integer inputs, truncation
  included — asserted, not toleranced.
- `vxn-asm-check` clean: the per-sample lane loop is untouched by all of this.
- The factory bank is re-levelled and auditioned once, not per-ticket.
