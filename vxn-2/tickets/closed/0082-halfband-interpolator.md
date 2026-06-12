---
id: "0082"
title: "Build halfband interpolation (upsampling) stage"
priority: high
created: 2026-06-12
epic: E007
depends: ["0081"]
---

## Summary

Third ticket of [E007](../../epics/open/E007-optional-per-voice-filter.md), and
the one genuinely new piece of DSP. VXN1 only ever decimates — it generates
oscillators directly at the oversampled rate, so it never built an interpolator.
VXN2 keeps the FM at base rate and oversamples *only* the filter (ADR 0004 §3),
so each voice must be **upsampled** before the ladder. Build the interpolating
counterpart to the decimator ported in [0081](0081-port-halfband-decimator.md).

## Design

- Add an interpolating path to `vxn2-dsp/src/halfband.rs` mirroring `Oversampler`:
  zero-stuff by 2 + the same symmetric halfband FIR, with ×2 gain compensation
  per stage (zero-stuffing halves passband energy). Cascade three 2× stages for
  2×/4×/8×, mirroring the decimator's A/B/C structure so a given stage always
  runs at a fixed rate (state stays coherent).
- API: `Oversampler::interpolate(input, output, factor)` (or a sibling
  `HalfbandInterp`), `input.len() * factor == output.len()`, `factor ∈ {1,2,4,8}`,
  1× = passthrough copy. `reset` + `clone_state_from` for parity with the
  decimator.
- The interpolation low-pass is **mandatory**, not optional: without it the
  base-rate spectral images survive into the ladder and intermodulate through
  its `tanh` (ADR 0004 §3).
- Reuse `DEFAULT_TAPS` / `DEFAULT_CENTRE`; do not introduce a second tap table.

## Acceptance criteria

- [x] `interpolate` produces `output.len() == input.len() * factor` for
  `factor ∈ {1,2,4,8}`; 1× is bit-identity.
- [x] DC gain ≈ 1 across the interpolated block (gain compensation correct, not
  1/F or F).
- [x] A base-rate tone well below Nyquist round-trips
  `interpolate → decimate` to within passband ripple (~0.1 dB) and group-delay
  shift; imaging products suppressed > 60 dB in the upsampled spectrum through
  the bulk of the passband. **Caveat:** this 33-tap halfband (the decimator's
  tap set) only holds >60 dB through ~16 kHz; the top transition band rolls off
  shallower (~−33 dB at 20 kHz) — inherent to the ported FIR, matches the
  decimator. Test validates the floor at a mid-passband tone (16 kHz → −66 dB);
  a missing LP would leave the image at ~0 dB.
- [x] Allocation-free, panic-free, no new dependencies.

## Notes

Round-trip group delay (interp + decimate) is what [0086](0086-latency-reporting.md)
reports to the host. Keep the per-stage delay queryable so the integration
([0084](0084-per-stack-filter-render-path.md)) can sum interp + decimate delay
per factor.
