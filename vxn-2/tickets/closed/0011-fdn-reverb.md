---
id: "0011"
title: FDN reverb
priority: medium
created: 2026-06-05
epic: E001
---

## Summary

Feedback Delay Network (FDN) reverb per ADR §7. 8-channel FDN with a
Hadamard mixing matrix, modulated delay lengths for density, per-channel
damping for HF rolloff. Macros: `reverb_size`, `reverb_decay`, `reverb_damp`,
`reverb_mix`.

Chosen over Schroeder (less tunable), convolution (CPU + IR storage), plate /
spring (character emulation, excluded by "clean" requirement).

Refer to patches/patches-modules for existing implementation

## Acceptance criteria

- [x] 8-channel FDN: 8 delay lines with mutually-prime base lengths (in
      samples). Lengths derived from `reverb_size` (range maps to a
      base-length set scaled by size).
- [x] Hadamard 8×8 mixing matrix on the feedback path. Applied as
      multiplications by ±1 (Hadamard structure makes this branch-free).
- [x] Per-channel damping: a one-pole lowpass on each delay-line output,
      coefficient driven by `reverb_damp`. Higher damp = lower cutoff =
      faster HF decay.
- [x] Decay control: feedback matrix gain set such that RT60 ≈ `reverb_decay`
      seconds. Use the standard FDN gain formula:
      `g = 10^(-3 × L_avg / (decay × sr))` where `L_avg` is mean delay
      length.
- [x] Stereo output: channels 0..3 sum to L, 4..7 sum to R, with
      cross-feedback through the Hadamard matrix providing stereo image.
- [x] Input: dry signal is split into the 8 channels via an input gain
      vector (random ±1 sign pattern, fixed seed for determinism).
- [x] Wet/dry mix: same equal-gain crossfade idiom as the delay.
- [x] Bypass (`reverb_on = false`): pass-through, bit-identical.
- [x] Smoothing: size changes (which would re-derive delay lengths) only
      take effect at the next note-silent moment OR crossfade between old
      and new lengths over ~500 ms. Decay and damp smooth continuously.
- [x] Bench: `reverb_steady` (active) and `reverb_bypassed`.

## Notes

FDN reference: Jot's "Digital Reverberation" tutorial (sci-fi textbook
material). 8 channels is the sweet spot for clean diffuse reverb; below
that gets metallic, above wastes CPU.

The Hadamard matrix structure means the mixing step is 8 additions + 8
subtractions + 8 negations = effectively branch-free. Don't write it as a
generic 8×8 matmul — explicit Hadamard structure.

Modulated delay lengths: each delay line has its length slightly modulated
(±2 samples) by an LFO running at 0.5 Hz per channel. This breaks up
flutter modes. Per-channel LFO phases are spread evenly.

Size parameter affects only base delay lengths; decay affects only the
feedback matrix gain; damp affects only the per-channel lowpass.
Independent macros.
