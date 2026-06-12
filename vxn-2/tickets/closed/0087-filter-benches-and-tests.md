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

- [x] Off-path cost is within noise of pre-epic baseline (no measurable
  regression) and bypass render is bit-identical.
- [x] On-path cost documented at F ∈ {1,2,4,8}; full-poly remains real-time at
  the chosen default factor.
- [x] Deferred-decimation equivalence test passes within FIR tolerance.
- [x] Aliasing/THD strictly decreases 1× → 2× → 4× → 8× on the driven resonant
  sweep; dB figures recorded in the test or `PARAMETERS.md`/README.
- [x] Self-oscillation bounded at every F; mode/slope response tests pass on the
  integrated path.
- [x] Quiescence-skip saving quantified; tail-preservation and skip-equivalence
  tests pass.
- [x] CI green including the new benches' smoke run.

## Notes

The deferred-decimation equivalence test is the load-bearing correctness check
for ADR 0004 §4 (summing and decimation commute) — it is what justifies the
single shared decimator over per-voice ones. Keep it explicit and well-named.

## Outcome (closed 2026-06-12)

**Benches** (`vxn2-osc-bench/benches/filter_path.rs`): off vs on at F∈{1,2,4,8}
plus a `filter_quiescence` group (sustaining vs released-and-rung-out at 4×).
Recorded RT-multiples (M-series, 48 kHz, 256-block, full poly 16×density-4, FX):
off 18.6×, 1× 10.2×, 2× 6.6×, 4× 4.4×, 8× 2.4× — all real-time. Quiescence-skip
reclaims ~99% of filter cost (1.24 ms held → 12 µs rung-out). Figures in the
bench header; CI runs `cargo bench --no-run --workspace` as the smoke compile.

**Tests:**

- Bypass bit-identity + self-osc-bounded-every-F + matrix cutoff/reso RT
  hardening + resonant-tail-no-skip-cliff →
  `vxn2-engine/tests/filter_integration.rs` (over the default patch + every
  factory preset for bypass).
- Aliasing/THD monotonic 1×→8× (−54.6 / −64.7 / −67.1 / −75.1 dB), integrated
  mode/slope, self-osc-bounded-every-F on the interp→ladder@F→decimate chain →
  `vxn2-dsp/src/filter.rs::tests`.
- Deferred-decimation equivalence: pre-existing `decimate_is_linear_over_voice_sum`
  in `vxn2-dsp/src/halfband.rs::tests` (explicit + well-named, < 1e-5 diff).
- Mode/slope, kernel self-osc, quiescence-decay: pre-existing
  `vxn2-dsp/src/filter.rs::tests`.
