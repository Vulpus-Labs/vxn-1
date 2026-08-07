---
id: "0251"
product: vxn-1b
title: "Wire the O/S and Limit selectors — port VXN1's output stage + master limiter"
priority: high
created: 2026-08-07
epic: E039
depends: ["0219"]
---

## Summary

The Master panel's **O/S** and **Limit** controls do nothing. Both params exist
in the table and are read by no one:

- `limiter_on` ([params.rs:589](../../vxn-1b/crates/vxn1b-engine/src/params.rs#L589))
  has no consumer anywhere — `FxChain` holds no limiter, and the engine's master
  stage is a bare volume multiply.
- `oversample` has a `Params::oversample_factor()` helper that nothing calls;
  `build_ctx` hardcodes `os: 1` with a "deferred" comment, so every voice renders
  at the base rate.

The O/S half is not just a dead control. Both synths default the param to **2×**
(index 1), so VXN1b has been rendering 1× while its own UI reads 2× — it aliases
more than VXN1 on the same patch. The render-parity gate missed this because it
forces VXN1's O/S to *off* before comparing.

## Design

**Limiter.** `StereoLimiter` is already shared in `vxn-core-utils` (per-sample
`process` plus a block wrapper). It goes in the **engine**, not `FxChain`: VXN1
applies master volume *before* its FX chain so the limiter is genuinely last,
whereas VXN1b applies master volume *after* the chain. Placing it in the chain
would let a master boost clip past the ceiling. So: after the master multiply and
the finite guard, before the master-out meter tap — the meter then reads what
actually leaves the plugin. Off→on edge resets the lookahead (stale transient);
the toggle crossfades dry↔limited over the FX fade window so engaging it can't
step level.

**Oversampling.** Port VXN1's `OutputStage` into a new `output.rs`, adapted to
the two-synth structure. It carries four behaviours worth having, all of them
learned from shipped bugs:

- **Decimator pair** (`Oversampler`, shared) — L and R, so spread survives.
- **`spread == 0` skip** (0107) — both layers centred ⇒ L == R bit-for-bit, so
  the R decimator is skipped and R is copied from L; the mono→stereo transition
  seeds R from L's converged state to avoid a click.
- **Silent-drain skip** (0106) — after `DECIMATOR_DRAIN_BLOCKS` fully-silent
  blocks the FIR has flushed, so decimation is skipped and the output zero-filled.
- **OS-change crossfade** (0191) — the factor change resets the rate-specific FIR,
  which then emits near-zero for its first samples; crossfade from the frozen
  pre-switch level into the rebuilt output so the join is continuous.

The bank render loop is *already* OS-aware (`base_frames = out.len() / os`, inner
`for k in 0..os`), so the work is threading `os` through `build_ctx` (giving
`os_sample_rate = sample_rate · os`) and giving the engine OS-rate buses.

**Per-layer gain under OS.** The layer mixer's gain smoothers must keep ramping
at the *base* rate, not the OS rate, or a fader move gets 2–8× faster as the
factor rises. Tick once per base frame and hold across that frame's OS
sub-samples.

This duplicates ~300 lines that [[0233]]/[[0235]] plan to share as `OsRegion` in
`vxn-core-dsp`; deliberate — those tickets then collapse both synths onto it,
rather than VXN1b's control staying dead until an extraction that rewrites VXN1's
shipping output stage lands.

## Acceptance criteria

- [ ] Limit audibly limits: a hot signal peaks at the ceiling with it on, above
      it with it off; toggling doesn't click; re-engaging leaks no stale transient.
- [ ] Limiter sits after master volume — raising master with Limit on cannot
      push the output past the ceiling.
- [ ] O/S 2×/4×/8× measurably reduce aliasing on a hard-sync / FM patch
      (non-harmonic energy below the fundamental drops as the factor rises).
- [ ] O/S off is bit-identical to today's render (the parity gate still passes).
- [ ] Changing factor mid-render doesn't click; both decimators reset on it.
- [ ] `spread == 0` still yields L == R bit-for-bit at every factor.
- [ ] Layer-fader ramp time is independent of the O/S factor.
- [ ] Hot path stays allocation-free (`tests/alloc_free.rs`).

## Notes

- VXN1's parity harness disables O/S on *both* sides; keep that, and add a
  separate aliasing test rather than widening the parity tolerance.
- Latency: VXN1 reports none for its decimator either, and
  [[vxn2-filter-epic]] records that reporting OS latency via a CLAP restart
  caused an audible dropout on every factor change. Don't report it here.
- The limiter's threshold is fixed (≈ −0.4 dBFS master ceiling) — no param,
  matching VXN1.
