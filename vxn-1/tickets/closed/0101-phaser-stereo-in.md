---
id: "0101"
title: Phaser — stereo-in process variant
priority: medium
created: 2026-06-07
epic: E019
---

## Summary

Add a `process_block_stereo(l_in, r_in, l_out, r_out)` method to
`StereoPhaser` so it can accept an already-stereo bus from the
engine in Stereo routing mode. Duplicates the allpass-cascade state
for L and R; shares the existing LFO (anti-phase offset preserved).

The existing `process_block(mono_in, l_out, r_out)` stays — Mono
routing mode keeps using it unchanged.

## Acceptance criteria

- [ ] `StereoPhaser` gains a second allpass state struct (or the
      cascade is restructured to hold per-channel state arrays).
- [ ] New method `process_block_stereo(l_in: &[f32], r_in: &[f32],
      l_out: &mut [f32], r_out: &mut [f32])`. Control-block hoisting
      pattern matches the existing `process_block`.
- [ ] LFO is shared between L and R chains; the anti-phase offset
      that currently produces the L vs R modulation difference is
      preserved.
- [ ] Test: feed L=R input (e.g. impulse train), assert L and R
      outputs match the existing mono-in path's L channel sample for
      sample (modulo a tolerance for any extra L-channel state
      walking the new branch). If exact match isn't achievable
      because the new branch processes R independently, document why
      and verify L matches the legacy L channel.
- [ ] Test: feed L≠R input (impulse on L only, zero on R), assert
      L output non-zero, R output near zero — confirms parallel
      independence.
- [ ] `cargo test -p vxn-dsp` green.

## Notes

Lives in `crates/vxn-dsp/src/phaser.rs`. Host-rate only (matches
existing variant).

The existing per-sample loop is built around a shared allpass
cascade with stereo split via the anti-phase LFO. Duplicating the
cascade is the simplest path; if it makes the state structs too big
to be ergonomic, restructure as `[AllpassChain; 2]`.

No new parameter surface — same Rate/Depth/FB/Mix.
