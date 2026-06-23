---
id: "0118"
product: monorepo
title: Promote limiter + half-band + scalar tanh to vxn-core-utils
priority: high
created: 2026-06-23
epic: E027
---

## Summary

Three DSP primitives are byte-identical across `vxn-dsp` and
`vxn2-dsp` but live in neither shared crate. Promote each
into `vxn-core-utils`, then have both synths consume.
Behaviour-preserving. Distinct from `0117` (which consumes
modules already in core); these must move *into* core first.

1. **Limiter** — `PeakWindow`, `LimiterCore`,
   `StereoLimiter` (incl. constants `THRESHOLD`, `ATTACK_MS`,
   `RELEASE_MS`, `MAX_ATTACK_MS`, doc comments, and tests)
   are functionally byte-identical between
   `vxn-1/crates/vxn-dsp/src/limiter.rs` and
   `vxn-2/crates/vxn2-dsp/src/limiter.rs` (~460 lines). vxn-2
   also inlines its own `DelayLine`; vxn-1 imports
   `crate::DelayLine` — promote `DelayLine` too. Serial
   recurrence by design, no SIMD concern.
2. **Half-band oversampler** — `HalfbandFir` (struct,
   `process`, `push_sample`, `clone_state_from`,
   `DEFAULT_TAPS`, `DEFAULT_CENTRE`) and `Oversampler` (all
   stages, `decimate` arms) are identical in both
   `halfband.rs` (~200 shared lines). vxn-2's extra
   `HalfbandInterp` / `Interpolator` stay vxn-2-local; only
   the shared decimation half + `roundtrip_latency_base_
   samples` const-fn move.
3. **Scalar tanh** — the branched-scalar Padé(5,6)
   `fast_tanh` body is byte-identical
   (`vxn2-dsp/src/math.rs:17` vs `vxn-dsp/src/math.rs:41`);
   vxn-1's own comment already says "keep the two in sync".

## Acceptance criteria

- [ ] `vxn-core-utils` gains a `limiter` module
      (`PeakWindow`, `LimiterCore`, `StereoLimiter`,
      `DelayLine`), a `halfband` module (`HalfbandFir`,
      `Oversampler`, `DEFAULT_TAPS`, `DEFAULT_CENTRE`,
      `roundtrip_latency_base_samples`), and the scalar
      `fast_tanh` in a `math` module — each with the tests
      that currently live beside the vxn-1 copies.
- [ ] Both `vxn-dsp` and `vxn2-dsp` import from
      `vxn-core-utils` and delete their copies; vxn-2 keeps
      `HalfbandInterp`/`Interpolator` locally, layered on the
      shared FIR.
- [ ] The branchless **poly-lane** tanh in
      `vxn-dsp/src/poly/oscillator.rs` is **not** moved or
      merged — add a comment pointing at the shared scalar
      one and noting the split is deliberate for
      vectorisation.
- [ ] `grep` finds one definition each of `LimiterCore`,
      `StereoLimiter`, `HalfbandFir`, `Oversampler`, scalar
      `fast_tanh`.
- [ ] `cargo test --workspace` green; both synths'
      `tests/baseline.rs` render hashes unchanged.

## Notes

The FIR inner loop is an autovectorisation candidate but
contains no handwritten SIMD — moving it does not affect
codegen. Land with or just after `0117` (both touch
`vxn-core-utils`; sequence to avoid churn). Memory
`vxn1-tanh-branchless-only`: only branch-free tanh variants
matter in the poly hot path — leave that one alone.
