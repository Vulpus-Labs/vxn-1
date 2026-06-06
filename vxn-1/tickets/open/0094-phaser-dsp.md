---
id: "0094"
title: Phaser DSP — port VStereoPhaser core into vxn-dsp
priority: medium
created: 2026-06-06
epic: E018
---

## Summary

Port the stereo allpass phaser from
`patches-bundles/patches-vintage::VStereoPhaser` into
`vxn-dsp::phaser` as a host-rate `StereoPhaser` kernel with a
collapsed macro surface: Rate / Depth / FB / Mix.

The upstream is two `PhaserChannel` cascades sharing one LFO,
right channel swept at a `spread` phase offset, signed
feedback, optional jitter, configurable allpass stages. The
vxn-1 port pins:

- `stages = 4` (4 allpass per channel → 2 notches)
- `spread = 1.0` (antiphase swirl)
- `width = 1.0` (neutral)
- `jitter = 0.0` (deterministic; drift handled at master level)
- `center_hz = 600` (mid-band)

…and exposes only Rate / Depth / FB / Mix to the engine.

Tests port verbatim from `vstereo_phaser/tests.rs` and
`vphaser/tests.rs`, adapted to the collapsed surface (skip
spread/width/jitter sweeps that aren't user-reachable).

## Acceptance criteria

- [ ] New `crates/vxn-dsp/src/phaser.rs` exporting
      `StereoPhaser` with `new(sample_rate)`,
      `set_params(rate_hz, depth, feedback, mix)`, `tick(in_l,
      in_r) -> (f32, f32)`, `reset()`.
- [ ] `mod phaser;` + `pub use phaser::StereoPhaser` added to
      `crates/vxn-dsp/src/lib.rs`.
- [ ] No RT allocation (verify with `patches-alloc-trap` or
      `cargo test` under the workspace's RT trap if one exists).
- [ ] `Rate ∈ [0.05, 10.0]` Hz, `Depth ∈ [0.0, 1.0]`,
      `Feedback ∈ [-0.9, 0.9]`, `Mix ∈ [0.0, 1.0]`.
- [ ] Param changes apply via vxn-dsp's smoother (or per-tick
      lerp) so knob sweeps don't zipper.
- [ ] Ported tests pass: identity at `mix=0`, audible notch
      sweep at `depth>0`, stable at `feedback ±0.85`, no NaN
      after 10 s of swept input.
- [ ] `cargo test -p vxn-dsp phaser::` green.

## Notes

The upstream allpass core (`vphaser/core.rs`) uses a one-pole
allpass per stage:

```text
y = -x * g + xh
xh_next = x + y * g
```

where `g = (1 - tan(π·fc/sr)) / (1 + tan(π·fc/sr))`. Triangle
LFO sweeps `fc` in log domain around `center_hz` by `depth · 2`
octaves. Feedback taps the last stage back into the first with
a one-sample delay (no zero-delay loop).

The vxn1 `StereoPhaser` should expose only the collapsed
surface — keep internal helper structs (`PhaserChannel`, the
LFO, the feedback delay) private to the module. If the channel
struct generalises later for a mono variant, promote then.

Don't pull `patches_sdk` deps. Lift only the DSP arithmetic.
Reference the upstream comments for the math but inline
constants — no `patches-sdk` import.

If the vxn-dsp `Smoothed` doesn't expose what's needed, use a
local `Smoother` like `chorus.rs` does — match local idiom over
introducing a shared helper.
