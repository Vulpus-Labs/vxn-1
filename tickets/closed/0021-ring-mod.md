---
id: "0021"
title: Poly ring modulator + drop brown noise
priority: medium
created: 2026-05-25
epic: E006
---

## Summary

Add a **ring modulator** (osc1 × osc2) using the Parker diode-bridge model, mixed
into the voice output by a new **RingLevel** alongside osc1/osc2/noise. Also
**drop brown noise** so `NoiseColor` is White/Pink only (matches the two-button
mixer selector in 0023).

Depends on **0022** for the `RingLevel` param + trimmed `NoiseColor` table entry
(0022 owns the param-table rewrite); this ticket is the DSP + voice wiring.

## Reference (patches-modules::modulators::ring_mod)

Julian Parker, DAFx-11 "A Simple Digital Model of the Diode-Based
Ring-Modulator":

```text
out = diode_block(signal + carrier·0.5) − diode_block(signal − carrier·0.5)
diode_block(x) = diode(x) + diode(−x)
diode(x) = x ≤ 0 ? 0 : tanh(poly5(x·gain)) / gain      // poly5 = Parker I–V fit
gain = 10^(drive_dB / 20)
```

`drive` (dB) sets the diode operating point: low ≈ near-ideal multiply, high =
harmonic colouring.

## Design

- **Poly (SoA) port** in `vxn-dsp`: a `PolyRingMod` (or a free function over
  `[f32; N]`) applying `diode_block` per voice, branchless where possible. The
  patches reference is mono; the poly fit is the same scalar maths per lane.
- **Inputs** = osc1 and osc2 per-voice samples (the same `o1`/`o2` the mix path
  already has). **Output** summed with `RingLevel` next to `Osc1Level` /
  `Osc2Level` / `NoiseLevel` in the voice mixer.
- **Drive**: start with a fixed/default diode drive; expose a `RingDrive` param
  only if 0023 wants a knob (epic leaves it out of the panel — confirm before
  adding a param). Keep ring contribution zero when `RingLevel = 0` (fast path).
- **Drop brown noise:** `NoiseColor` enum → `{White, Pink}`; remove the Brown
  arm in `PolyNoise::process` and the `brown` state. `NOISE_LABELS` already
  needs to be `["White","Pink"]` (0022).

## Acceptance criteria

- [ ] Ring output = Parker diode model of osc1×osc2; zero on either input ⇒
      ~silence (mirror patches' `zero_carrier_silences_output` /
      `zero_signal_silences_output`).
- [ ] `RingLevel` mixes the ring signal in alongside osc1/osc2/noise;
      `RingLevel = 0` is the cheap no-op path.
- [ ] Output finite across all lanes incl. frozen voices; drive shapes colour.
- [ ] Brown noise removed; `NoiseColor` is White/Pink; no dead enum index.
- [ ] No RT allocation; lane loop stays finite/vectorisable.

## Notes

- Ring mod is generally aliasing-prone at high carrier ratios; like sync/FM it
  leans on the engine oversampling for v1.
- Validation: `cargo test -p vxn-dsp -p vxn-engine`.
