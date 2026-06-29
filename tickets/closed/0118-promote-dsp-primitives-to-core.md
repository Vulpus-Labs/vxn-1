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

## Close-out (2026-06-26)

- **Limiter.** New [limiter.rs](../../crates/vxn-core-utils/src/limiter.rs)
  in `vxn-core-utils` holds `PeakWindow`, `LimiterCore`, `StereoLimiter` and a
  private inlined `DelayLine` (+ `THRESHOLD`/`ATTACK_MS`/`RELEASE_MS`/
  `MAX_ATTACK_MS` and the 7 unit tests). Verified the two synths' copies were
  byte-identical apart from docs/imports and vxn-2's inlined-vs-imported
  `DelayLine` (whose `new`/`clear`/`write`/`read` match vxn-1's
  `delay.rs::DelayLine` exactly). `vxn-dsp/src/limiter.rs` and
  `vxn2-dsp/src/limiter.rs` are now re-export shims; engine call sites
  (`vxn-engine`, `vxn2-engine`) unchanged.
- **Half-band.** New [halfband.rs](../../crates/vxn-core-utils/src/halfband.rs)
  holds the decimation half: `HalfbandFir`, `Oversampler`, `DEFAULT_TAPS`,
  `DEFAULT_CENTRE`, `roundtrip_latency_base_samples` (+ decimator tests). The
  `HalfbandFir`/`Oversampler` code was identical across both synths (confirmed
  comment-stripped diff). `vxn-dsp/src/halfband.rs` is a pure re-export;
  `vxn2-dsp/src/halfband.rs` re-exports the shared half and keeps its
  `HalfbandInterp`/`Interpolator` (+ interp tests) local, layered on the shared
  FIR via the re-exported `HalfbandFir::GROUP_DELAY_OVERSAMPLED`.
- **Scalar tanh.** New [math.rs](../../crates/vxn-core-utils/src/math.rs) holds
  `fast_tanh` (byte-identical Padé(5,6) body + tests). `vxn-dsp/src/math.rs`
  and `vxn2-dsp/src/math.rs` re-export it; both keep their other locals
  (`fast_exp2`/`fast_sine`/`lookup_sine`/`xorshift64` stay in vxn-1).
- **Poly-lane tanh left in place.** `vxn-dsp::poly::oscillator::tanh_c`
  ([oscillator.rs:53](../../vxn-1/crates/vxn-dsp/src/poly/oscillator.rs#L53))
  not moved/merged; its doc now points at the shared scalar `fast_tanh` and
  states the branchless-clamp split is deliberate for vectorisation.
- **Single-definition sweep.** `grep` across `crates/`, `vxn-1/crates/`,
  `vxn-2/crates/` finds exactly one definition of `LimiterCore`,
  `StereoLimiter`, `HalfbandFir`, `Oversampler`, and scalar `fast_tanh` (all in
  `vxn-core-utils`).
- **Tests.** `cargo test --workspace` green; `vxn-engine` `tests/baseline.rs`
  render hash unchanged (proves the promoted FIR/limiter are bit-identical);
  vxn-2 `filter_integration` (exercises the oversampler) green. `cargo clippy
  -p vxn-core-utils` clean.
