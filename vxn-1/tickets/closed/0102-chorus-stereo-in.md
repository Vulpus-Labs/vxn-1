---
id: "0102"
title: Chorus — stereo-in process variant
priority: medium
created: 2026-06-07
epic: E019
---

## Summary

Add `process_block_stereo(l_in, r_in, l_out, r_out)` to
`StereoChorus`. Second delay-line struct for the R channel; LFO and
control-block hoisting shared with the existing inverted-per-line
setup so the L/R modulation contrast is preserved.

Existing `process_block(mono_in, l_out, r_out)` stays — Mono
routing mode keeps using it unchanged.

## Acceptance criteria

- [ ] `StereoChorus` gains a second delay-line state for the R
      channel (or refactor to `[DelayLine; 2]` if cleaner).
- [ ] New method `process_block_stereo(l_in: &[f32], r_in: &[f32],
      l_out: &mut [f32], r_out: &mut [f32])` with the same
      control-block hoisting cadence as the existing variant.
- [ ] LFO state and inverted-LFO-per-line behaviour preserved.
- [ ] Test: L=R input (e.g. sine) → L and R outputs match the
      existing mono-in path's L channel within float tolerance, OR
      document the divergence (new R chain has its own delay-line
      state and won't perfectly reproduce the legacy R channel).
- [ ] Test: L≠R input (sine on L, zero on R) → L output carries the
      sine plus modulation; R output carries near-silence plus
      whatever the inverted LFO scatters across the new R delay line.
      Confirms parallel processing.
- [ ] `cargo test -p vxn-dsp` green.

## Notes

Lives in `crates/vxn-dsp/src/chorus.rs`.

The existing delay-line stores 1 mono input → reads with two LFO
phases for L and R outputs. New stereo path needs 2 delay lines
each with its own LFO read offset; the LFO oscillator itself stays
single-instance.

Wet/dry mix law unchanged.
