---
id: "0087"
title: "Filter benchmarks + tests: cost, bypass bit-identity, aliasing, quiescence"
priority: medium
created: 2026-06-12
epic: E007
depends: ["0084", "0085"]
---

## Summary

Final ticket of [E007](../../epics/open/E007-optional-per-voice-filter.md).
Lock the filter feature behind benchmarks and tests: prove the off path is free,
the on path is bounded and correct, oversampling actually reduces aliasing, and
quiescence-skip is both safe and a real saving. Extends the `vxn2-osc-bench`
suite and the engine integration tests.

## Design

Benchmarks (extend `vxn2-osc-bench`):

- Filter **off** vs **on** full-poly render cost; on-path cost at F ∈ {1,2,4,8}.
- Held chord with released resonant tails, skip on vs off, to quantify the
  quiescence saving.
- Document RT-multiple figures (Apple M-series, 44.1 kHz, 64-sample block)
  alongside the existing dry/sync/idle numbers.

Tests (engine integration + dsp unit):

- **Bypass bit-identity**: golden render of every factory patch with
  `filter-enable` off equals the pre-epic baseline, sample-for-sample.
- **Deferred-decimation equivalence**: a known multi-voice input, decimated
  once-post-sum, matches summing per-voice-decimated outputs within FIR
  tolerance.
- **Mode/slope response**: energy ordering for LP12 > LP24 in HF, HP/BP/Notch
  selectivity, on the integrated per-voice path (not just the kernel).
- **Aliasing/THD**: a driven, resonant cutoff sweep — measure inharmonic energy
  above base-Nyquist at 1× vs 2×/4×/8×; assert monotonic reduction and record
  the dB figures.
- **Self-osc stability**: resonance = 1 across the cutoff range stays finite and
  bounded at every F.
- **Quiescence safety**: resonant release tail preserved (0085 criteria) and
  settled voices skipped, output within tolerance of the always-filter path.
- **No RT alloc / no panic** across the process callback on both paths
  (reuse the existing RT-hardening harness).

## Acceptance criteria

- [ ] Off-path cost is within noise of pre-epic baseline (no measurable
  regression) and bypass render is bit-identical.
- [ ] On-path cost documented at F ∈ {1,2,4,8}; full-poly remains real-time at
  the chosen default factor.
- [ ] Deferred-decimation equivalence test passes within FIR tolerance.
- [ ] Aliasing/THD strictly decreases 1× → 2× → 4× → 8× on the driven resonant
  sweep; dB figures recorded in the test or `PARAMETERS.md`/README.
- [ ] Self-oscillation bounded at every F; mode/slope response tests pass on the
  integrated path.
- [ ] Quiescence-skip saving quantified; tail-preservation and skip-equivalence
  tests pass.
- [ ] CI green including the new benches' smoke run.

## Notes

The deferred-decimation equivalence test is the load-bearing correctness check
for ADR 0004 §4 (summing and decimation commute) — it is what justifies the
single shared decimator over per-voice ones. Keep it explicit and well-named.
