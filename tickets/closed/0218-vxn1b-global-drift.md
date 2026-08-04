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

## Close-out (2026-08-03)

- `MasterDrift` exposed **once**, in the global block
  ([params.rs:339](../../vxn-1b/crates/vxn1b-engine/src/params.rs#L339)), desc
  `master_drift` `[0,1]` linear default `0.0`
  ([params.rs:566](../../vxn-1b/crates/vxn1b-engine/src/params.rs#L566)); it is
  *not* in `PATCH_PARAMS` — `params::tests::patch_and_global_partition_every_param`
  proves the tables partition, so there is no per-layer duplicate. UI control is
  the Drift fader in the Master panel of Tab 3
  ([faceplate.html:323](../../vxn-1b/crates/vxn1b-ui-web/assets/faceplate.html#L323)).
- Global scope: `Engine::set_param`'s `ClapRef::Global` arm writes the inner id to
  **both** synths, and each synth reads it once per control block into
  `BlockCtx::drift_amount`
  ([synth.rs:314](../../vxn-1b/crates/vxn1b-engine/src/synth.rs#L314)).
- Osc pitch walk was already live (shared `vxn-dsp` `PolyOscillator::tick_drift`,
  per-osc salted seeds, ±0.125 st at 1.0) —
  [bank.rs:445](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L445).
- Component trims ported verbatim from VXN1's `VoiceTrim`
  ([voice.rs:65-140](../../vxn-1/crates/vxn-engine/src/voice.rs#L65-L140)): four
  salted SplitMix64 streams per bank seed
  ([bank.rs:88](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L88),
  [bank.rs:106](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L106)), applied to
  env A/D/R ±12% and sustain ±3%
  ([bank.rs:359](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L359)), base cutoff
  ±3 ¢ and resonance ±7%
  ([bank.rs:567](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L567)). Magnitudes
  pinned by `bank::tests::trim_magnitudes_match_vxn1`.
- Filter key-track now follows mean osc drift: `drift_key_track`
  ([bank.rs:139](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L139)) recovers
  VXN1's `filter_key_track` amount from the Key→Cutoff route
  (`Σdepth · DEST_GAIN[Cutoff] / 12`; `KEY_CUTOFF_UNITY_DEPTH` → 1.0), resolved
  once per block ([bank.rs:469](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L469)),
  replacing the `0.0 // trim deferred` stub. Test:
  `bank::tests::drift_key_track_reads_the_key_cutoff_route`.
- Drift automation is live: `MasterDrift` joins the envelope re-cook set
  ([synth.rs:335](../../vxn-1b/crates/vxn1b-engine/src/synth.rs#L335), renamed
  `is_envelope_param` → `recooks_envelopes`), since the trims scale the cooked
  ADSR params.
- Tests: `engine::tests::global_drift_reaches_both_layers` renders each synth in
  isolation — drift 0 bit-identical across repeats, drift 0.9 changes **both**
  layers' voices; `bank::tests::drift_spreads_envelope_times_across_lanes`
  (drift 0 → identical lanes, drift 1 → spread);
  `bank::tests::trims_change_output_with_the_oscillators_muted` isolates the
  filter/env trims from the pitch walk on a noise-only patch;
  `bank::tests::trim_properties` / `cutoff_trim_stays_in_tune`. The render-parity
  gate (`tests/parity.rs`) is unchanged — drift 0 collapses every trim to 1.0.
- Allocation-free: `tests/alloc_free.rs` now sets `MasterDrift` and renders
  inside the armed section (re-cook + drifted cutoff/reso path) — still 0 allocs.
- Shipped in `b7b15f9`.
