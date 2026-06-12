---
id: "0080"
title: "Port OTA-C ladder kernel + Padé fast_tanh into vxn2-dsp"
priority: high
created: 2026-06-12
epic: E007
depends: []
---

## Summary

First ticket of [E007](../../epics/open/E007-optional-per-voice-filter.md).
VXN2 has no filter and no `tanh` — the FM operators are pure sine. Bring VXN1's
OTA-C ladder into `vxn2-dsp` as a self-contained, dependency-free module so the
per-voice filter path ([0084](0084-per-stack-filter-render-path.md)) has a
kernel to call.

Source: [ota_ladder.rs](../../../vxn-1/crates/vxn-dsp/src/ota_ladder.rs) and the
Padé-(5,6) `fast_tanh` in [math.rs](../../../vxn-1/crates/vxn-dsp/src/math.rs).
`vxn2-dsp` is deliberately dependency-free (it *copies* VXN1 primitives — see
`smoother.rs`, "lifted from VXN1"); we port, we do not depend on the VXN1 crate.

## Design

- Add `vxn2-dsp/src/math.rs` with `fast_tanh` (Padé degree-5 numerator /
  degree-6 denominator, ±2.5 hard clamp). Keep the existing branch structure —
  the clamp branches are hot-path-sensitive per VXN1's `tanh-branchless-only`
  lesson; do not refactor the clamp, just port and re-measure later.
- Add `vxn2-dsp/src/filter.rs` with `OtaLadderKernel`, `OtaLadderCoeffs`,
  `FilterMode` (LP/HP/BP/Notch), `FilterSlope` (2/4-pole), `compute_g`, copied
  verbatim from VXN1. Scalar frozen-coefficient kernel only:
  `set_coeffs` once per block, `set_response`, `tick(x) -> y` per sample.
- Do **not** port `PolyOtaLadder` (the per-sample-ramped 8-lane SoA sibling) —
  the filter runs on a stack's *summed* stereo pair, so there is no per-lane SoA
  here. Two scalar kernels per stack (L/R) is the granularity.
- Do **not** port VXN1's Moog `ladder` or standalone `hpf` (out of epic scope).
- Register both modules in `vxn2-dsp/src/lib.rs`.
- Port the kernel's existing unit tests (DC pass / HF reject, mode-tap energy
  ordering, HP/BP/Notch selectivity, high-resonance stability).

## Acceptance criteria

- [x] `vxn2-dsp` compiles with `filter` + `math` modules and **no new
  dependencies** (`[dependencies]` stays empty).
- [x] `fast_tanh` matches VXN1's output bit-for-bit on a sweep over [−3, 3]
  (verbatim port of the Padé-(5,6) body + ±2.5 clamp).
- [x] Ported kernel tests pass: DC gain ≈ 1 at low cutoff, HF crushed, LP12
  brighter than LP24, HP/BP/Notch selectivity, self-osc bounded (`peak < 10`)
  at resonance = 1.
- [x] `tick` is `#[inline]`, allocation-free, panic-free; no `unwrap`/`expect`.
- [x] Public API documented; module header notes the OTA-C lineage and that it
  is lifted from VXN1.

## Notes

`OtaLadderCoeffs::new(cutoff_hz, sample_rate, resonance, drive)` takes resonance
in `[0, 1]` and scales to the `[0, 4]` feedback range internally (self-osc at
1.0) — keep that call convention; the param layer ([0083](0083-filter-params-and-matrix-dests.md))
feeds `[0, 1]` directly. `sample_rate` passed here is the **oversampled** rate
in the filter path, so `compute_g`'s `fs`-dependent pole detune stays correct.
