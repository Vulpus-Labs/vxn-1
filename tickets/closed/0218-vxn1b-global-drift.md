---
id: "0218"
product: vxn-1b
title: "Global drift — port VXN1 MasterDrift, one control for both synths"
priority: medium
created: 2026-07-31
epic: E039
depends: ["0214"]
---

## Summary

Port VXN1's global **drift** control into VXN1b, unchanged. A single global
`MasterDrift` amount applied to all voices in **both** synths. Per
[ADR 0002](../../vxn-1b/adrs/0002-vxn1b-dual-layer.md) §6.

## Design

Port verbatim from VXN1 — this is not a new feature, it's a straight lift:

- **`MasterDrift`** CLAP param `[0.0, 1.0]`, default `0.0`, linear
  ([vxn-app/src/params.rs:266,770](../../vxn-1/crates/vxn-app/src/params.rs#L266)).
  Lives in the **global block** (Tab 3), not per-layer.
- **Not a param selector** — a master amount scaling per-voice variance baked
  into DSP. Targets (no driftable-param list; hardcoded):
  - **Osc pitch**: per-voice bounded random walk, ±0.125 st @ 1.0, advances every
    2 control blocks (sub-Hz)
    ([vxn-dsp/src/poly/oscillator.rs:205,286-316](../../vxn-1/crates/vxn-dsp/src/poly/oscillator.rs#L286-L316)).
  - **Component trims** (static per-lane draws, scaled at apply):
    env A/D/R ±12%, sustain ±3%, resonance ±7%, cutoff ±3¢
    ([vxn-engine/src/voice.rs:65-140](../../vxn-1/crates/vxn-engine/src/voice.rs#L65-L140)).
  - Filter key-track follows mean osc drift (musical beating).
- **Global scope**: read once per control block, broadcast to **all voices in
  both `Synth`s**. `drift = 0` → trims collapse, voices bit-identical.

## Acceptance

- `MasterDrift` param exposed once (global), default 0.
- Both synths' voices drift from the one control; drift = 0 is bit-identical.
- Osc random walk + env/filter trims match VXN1 magnitudes.
- Test: drift > 0 produces per-voice pitch/timbre variance in both layers; drift
  = 0 produces none. Allocation-free.
