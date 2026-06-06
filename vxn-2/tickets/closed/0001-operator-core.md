---
id: "0001"
title: Operator core (osc + EG + key scaling + level)
priority: high
created: 2026-06-05
epic: E001
---

## Summary

Build the fundamental operator unit: a phase accumulator + sine generator + 4-segment
DX7-style EG + per-op level + key scaling + velocity / amp sensitivity + per-op
feedback. This is the atom every VXN2 voice is made of (6 per voice, up to 8×
stacking × 16 voices = 768 instances).

Lives in `vxn2-dsp::op` with a thin `OpParams` POD describing one op's runtime
state. The op runs per-sample inside a tight loop, given its current modulation
input (the sum of incoming PM from other ops per the algorithm graph). Existing
`vxn2-osc-bench` work informs the implementation: 3-op FM benches already
exercise a precursor.

## Acceptance criteria

- [x] `OpParams` struct holds all per-op runtime values from `PARAMETERS.md`.
- [x] `OpState` holds Q32 phase + EG state + KS-cached level scaler + per-op FB
      memory.
- [x] `op_tick(state, params, modulation_input, key) -> f32` runs branch-free,
      auto-vectorises in the same SoA pattern as VXN1 (verified via asm dump).
- [x] EG: 4 rates (R1..R4) × 4 levels (L1..L4), exact DX7-spec curve shape
      (per https://www.chipple.net/dx7/fig01-04.gif reference table).
- [x] Note-on / note-off transitions: attack from L4, gate-off jumps to release
      segment, release falls to L4.
- [x] Key scaling level: at `ks_break_pt` no scaling; left of BP, scales by
      `ks_l_depth` shaped by `ks_l_curve`; right by `ks_r_depth`/`ks_r_curve`.
      All four curve types (+lin, −lin, +exp, −exp) implemented.
- [x] Key scaling rate: scales all four EG rates by `ks_rate × note_delta`.
- [x] Velocity sensitivity (`vel_sens`, 0..7) attenuates `level` proportional to
      note-on velocity per DX7 table.
- [x] Amp sensitivity (`amp_sens`, 0..3) attenuates LFO depth applied to
      `level` per DX7 table (matrix-driven; this is the receive coefficient).
- [x] Per-op feedback: a `feedback` value > 0 adds the op's own previous output
      (averaged with one-sample-back to suppress aliasing per DX7 convention)
      to its phase input, scaled by `feedback` (DX7 scaling table).
- [x] Bench `vxn2-osc-bench` extended with `op_voice_steady` and
      `op_voice_attack` — single-op cost, idle and active.

## Notes

DSP reference: chipple.net DX7 ROM tables for EG curve shape, velocity, KS
depth scaling, and per-op FB averaging coefficient. Bhaskara+Moser sine per
project README. Q32 phase per project README + ADR 0001 §1.

The 3-op FM benches under `vxn2-osc-bench` use a precursor `Op` struct.
Promote it into `vxn2-dsp::op`, then update benches to use the promoted type
so we have a single source of truth.

EG and KS are the two hardest correctness items. Worth a dedicated test that
plays a held note across MIDI 0..127 at velocities 1..127 and asserts the
resulting envelope shape matches a captured baseline within tolerance.
