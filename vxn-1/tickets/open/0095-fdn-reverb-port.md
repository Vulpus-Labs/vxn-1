---
id: "0095"
title: FDN reverb — port vxn2 Reverb into vxn-dsp; delete bbd.rs
priority: medium
created: 2026-06-06
epic: E018
---

## Summary

Replace the BBD `StereoVReverb` with a port of the vxn2 FDN
reverb. The vxn2 source lives at
`vxn-2/crates/vxn2-dsp/src/reverb.rs` — 8 mutually-prime delay
lines, 8×8 Hadamard mix on feedback, per-line one-pole LP
damping, per-line ±2-sample LFO modulation. Port it as
`vxn-dsp::fdn_reverb::FdnReverb`.

**Scope correction:** `bbd.rs` is *not* deleted outright — the
chorus consumes `ModDelayLine`, `Interp`, and the BBD bank /
delay primitives. Trim `bbd.rs` to keep only those chorus
primitives and rip the reverb-only items (`StereoVReverb`,
`TappedDelayLine`, `AllpassDiffuser`, MN3011 tap constants,
reverb size/damp/mod helpers, and their tests).

The user surface collapses to four knobs: Size, Decay, Damp,
Mix. No Type selector.

## Acceptance criteria

- [ ] New `crates/vxn-dsp/src/fdn_reverb.rs` mirroring
      `vxn2-dsp/src/reverb.rs` structure, with the `Smoothed`
      import swapped for vxn-dsp's smoother (or local
      equivalent — match `chorus.rs` / `delay.rs` idiom).
- [ ] Constants preserved: `LINES = 8`, `BASE_MS`,
      `INPUT_SIGN`, `LFO_HZ`, `LFO_DEPTH_SAMP`, `INV_SQRT8`,
      `SIZE_SMOOTH_MS`, `MIN_SIZE_SCALE`, `MAX_SIZE_SCALE`.
- [ ] Public API: `new(sample_rate)`,
      `set_params(size, decay, damp, mix)`,
      `tick(in_l, in_r) -> (f32, f32)`, `reset()`.
- [ ] `on = false` returns input unchanged with no buffer work
      (matches vxn2's bypass semantic).
- [ ] `crates/vxn-dsp/src/bbd.rs` retains only the chorus-shared
      primitives (Complex32, ContinuousPoleBank, recon_bank,
      default_pole_pairs, normalised_pair_residues, OnePoleLpf,
      DelayBuffer, ThiranInterp, Interp, ModDelayLine). All
      reverb-specific items removed (StereoVReverb,
      TappedDelayLine, AllpassDiffuser, REVERB_*, MN3011_*,
      DIFFUSER_*, size_to_delay_s, damping_fc_hz, mod_rate_hz,
      reverb_triangle, and the reverb tests).
- [ ] `crates/vxn-dsp/src/lib.rs`: `pub use bbd::StereoVReverb`
      removed. New `mod fdn_reverb; pub use
      fdn_reverb::FdnReverb`.
- [ ] `random_walk.rs` keeps working — it has no `bbd.rs`
      dependency.
- [ ] `cargo test -p vxn-dsp` green. Port the vxn2 reverb tests
      with path-only adjustments.
- [ ] `grep -r 'StereoVReverb\|MN3011\|TappedDelayLine\|AllpassDiffuser' vxn-1/crates`
      returns zero matches after the port.
- [ ] Engine wiring updated: `Synth::reverb` field type swaps to
      `FdnReverb`; `update_effects` derives size/decay/damp from
      the existing Type+Depth macro for now (full param-table
      churn is 0096) and feeds FDN's own mix internally; the
      external wet/dry crossfade is removed.

## Notes

The vxn2 module is host-rate clean Rust with no SIMD intrinsics
— the lane loop is `for l in 0..LINES` and the compiler
vectorises it (per [[vxn1-soa-match-defeats-simd]] caveats,
which don't apply here because no enum dispatch lives inside
the lane loop).

InterleaveRing layout (AoS, one row = `[f32; LINES]` per slot)
is right for FDN — the Hadamard mix touches all 8 lines per
sample, so AoS gives unit-stride loads. Keep it.

Hadamard mix is a 24-op fast Walsh-Hadamard (see vxn2 source
lines ~170+). Don't optimise.

Macros land in the engine, not here:
- `reverb_size` smooths over ~500 ms internally (matches the
  vxn2 ticket AC; size changes glide).
- `reverb_decay` is RT60 — engine just passes the value through.
- `reverb_damp` drives the per-line LP cutoff — engine passes
  through.

If the vxn-dsp tree has no `smoother::Smoothed`, lift the vxn2
one (`vxn-2/crates/vxn2-dsp/src/smoother.rs`) as
`vxn-dsp::smoother` in the same ticket — small file, no
upstream baggage.

Engine wiring + bus position lives in 0097, not here.

## Closes / supersedes

- [E012](../../epics/open/E012-vreverb-port.md) — the BBD reverb
  this swaps out. Move E012 to `closed/` once 0095 lands.
- [0059 — Factory reverb tasting](0059-vreverb-factory-tasting.md)
  — close as superseded by 0099 (factory audit for FDN).
