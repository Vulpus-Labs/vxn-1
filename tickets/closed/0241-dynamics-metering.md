---
id: "0241"
product: vxn-1b
title: "Dynamics metering — in / out stereo levels + gain reduction on the FX block"
priority: medium
created: 2026-08-03
epic: E039
depends: ["0240", "0220"]
---

## Summary

Meter the Dynamics panel on the FX/Global tab: **inbound stereo level**,
**outbound stereo level**, and **gain reduction**, over the [[0240]] metering
spine. Gives the compressor a readable working display instead of ear-only
threshold/ratio setting.

**Scope grew** during 0220's layout pass: an *output* meter was added alongside
in + GR, so the panel shows what the block is doing to the level (makeup
included) rather than only how hard it is pulling. Built together with the
layout rework, since the Dynamics panel moved to the top row at the same time.

## Design

- **Inbound level**: stereo peak tapped at the dynamics slot's *input* — i.e.
  the summed layers before the compressor, and before the rest of the serial
  chain, since dynamics runs **first** ([ADR 0001](../../vxn-1b/adrs/0001-vxn1b-overall-design.md) §8).
  Two `MeterBus` slots, atomic-max, exactly as [[0240]].
- **Outbound level**: stereo peak tapped at the slot's *output* — post comp/sat
  **and post the bypass crossfade**, so it reads what the slot actually hands to
  the chorus rather than the kernel's raw wet.
- **Gain reduction**: one slot, **not two**.
  [`DynamicsBlock`](../../vxn-1/crates/vxn-dsp/src/dynamics.rs#L216) runs a
  single stereo-linked detector and already computes `gr_db` per sample
  internally ([dynamics.rs:250](../../vxn-1/crates/vxn-dsp/src/dynamics.rs#L250));
  it is one control signal, so a second channel would just duplicate it.
  Publish as atomic-**min** (most negative dB = deepest reduction) on a slot
  initialised to `0.0`, read-and-clear like the peak slots.
- **vxn-dsp change**: expose the reduction. Additive getter (`last_gr_db()`, or
  a block-min accumulator) on `DynamicsBlock` — shared with vxn-1 and vxn-2, so
  it must not change existing behaviour or the process signature.
- **UI**: the reduction bar reads **downward from 0 dB** (standard compressor
  idiom), beside the inbound stereo pair, in the Dynamics panel.
- Reduction reads `0.0` while the slot is bypassed / steady-off — the
  [true-skip gate](../../vxn-1b/crates/vxn1b-engine/src/fx.rs) never calls
  `process`, so nothing publishes and read-and-clear yields `0.0`.

## Acceptance criteria

- [x] `DynamicsBlock` exposes gain reduction additively (`take_gain_reduction_db`,
      read-and-clear, branch-free per-sample `min`); vxn-1 (163) and vxn-2 (216)
      engine tests and vxn-dsp (92) pass unchanged.
- [x] Dynamics in + out stereo peaks + GR published to the `MeterBus`;
      allocation-free (peaks accumulate in locals, one atomic per tap per block).
- [x] Dynamics panel shows In / GR / Out — signal order left to right, GR drawn
      downward from 0 dB.
- [x] GR reads 0 when the slot is off or the signal is below threshold
      (`dynamics_meters_report_in_out_and_reduction`).
- [x] Contract/token tests pass; loads without JS errors.
- [x] Opens in a DAW — verified in Reaper 2026-08-26.

## Notes

- Deliberately **after** [[0220]]: the mixer meters exercise the spine's
  multi-tap path first, and the GR slot is the only tap needing a new atomic
  discipline (min, not max), so it lands on a proven bus.

## Close-out (2026-08-26)

- `DynamicsBlock::take_gain_reduction_db` added additively to the shared kernel
  ([dynamics.rs:159](../../vxn-1/crates/vxn-dsp/src/dynamics.rs#L159)) — read-and-clear
  block-min, branch-free per sample, process signature unchanged, so vxn-1 and
  vxn-2 are unaffected.
- Five taps published: `DynamicsInL/R` at the slot input (pre-comp, and first in
  the serial chain per ADR 0001 §8), `DynamicsOutL/R` post comp/sat **and** post
  the bypass crossfade, and the single stereo-linked `DynamicsGr` on atomic-min.
  Peaks accumulate in locals, one atomic per tap per block — allocation-free.
- Dynamics panel draws In / GR / Out left to right, GR downward from 0 dB.
- GR reads exactly 0 when the slot is off or the signal is under threshold — the
  true-skip gate never calls `process`, so nothing publishes and read-and-clear
  yields 0. Pinned by
  [`dynamics_meters_report_in_out_and_reduction`](../../vxn-1b/crates/vxn1b-engine/src/engine.rs#L1318).
- Verified in Reaper: in / out / GR all track the compressor under load.
