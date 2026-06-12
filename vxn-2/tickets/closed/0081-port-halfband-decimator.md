---
id: "0081"
title: "Port halfband decimator (Oversampler) into vxn2-dsp"
priority: high
created: 2026-06-12
epic: E007
depends: []
---

## Summary

Second ticket of [E007](../../epics/open/E007-optional-per-voice-filter.md).
The deferred-decimation design (ADR 0004 §4) needs one shared 2×/4×/8× halfband
decimator running on the oversampled voice-sum bus. Port VXN1's `HalfbandFir` +
`Oversampler` from [halfband.rs](../../../vxn-1/crates/vxn-dsp/src/halfband.rs)
into `vxn2-dsp` verbatim — it is already a clean, dependency-free FIR.

## Design

- Add `vxn2-dsp/src/halfband.rs` with `HalfbandFir` (33-tap symmetric
  linear-phase, `DEFAULT_TAPS` + `DEFAULT_CENTRE`) and `Oversampler` (three
  cascaded 2:1 stages → 2×/4×/8×, 1× = passthrough copy), copied as-is.
- Keep `HalfbandFir::GROUP_DELAY_OVERSAMPLED = 16` and `reset` /
  `clone_state_from` — [0086](0086-latency-reporting.md) reads the group delay
  for latency reporting; `clone_state_from` warm-starts the R-channel decimator
  from L on a mono→stereo transition.
- Register in `vxn2-dsp/src/lib.rs`. Port the existing tests (DC pass-through and
  Nyquist rejection at 2×/4×/8×, 1× identity).

## Acceptance criteria

- [x] `Oversampler::decimate(input, output, factor)` ported; `factor ∈ {1,2,4,8}`,
  `input.len() == output.len() * factor`.
- [x] Ported tests pass: DC gain ≈ 1 and oversampled-Nyquist leakage < 0.05 at
  2×/4×/8×; 1× is bit-identity.
- [x] No new `vxn2-dsp` dependencies; allocation-free `process`; panic-free.

## Notes

VXN1's docstring notes it ships *only* a decimator (oscillators run at the OS
rate there). VXN2 needs the interpolating counterpart too —
[0082](0082-halfband-interpolator.md) builds it on top of this module so both
directions share the same tap table and cascade structure.
