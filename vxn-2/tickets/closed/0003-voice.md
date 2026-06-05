---
id: "0003"
title: Voice (6 ops + voice-level state, mono signal path)
priority: high
created: 2026-06-05
epic: E001
---

## Summary

Compose six operators (0001) + an algorithm router (0002) into a single voice
that produces a mono output sample per call given the current played note,
velocity, and pre-resolved parameter snapshot. Includes per-voice modulation
state — Pitch EG and Mod Env aren't built yet (0007), but their hookups are
stubbed so 0007 plugs in without API churn.

The voice is the unit that gets allocated to a played note (0004 will
allocate, 0005 will stack).

## Acceptance criteria

- [ ] `Voice` struct holds: 6× `OpState` (0001), played note, velocity,
      gate, current pitch (note + bend + glide), stack-instance metadata
      (`voice_idx`, `voice_spread`, `voice_rand` — populated by 0005,
      defaulted in this ticket).
- [ ] `voice_tick(voice, patch, modulation) -> f32` returns one sample.
      `modulation` is a per-block resolved struct (mod matrix output; stubbed
      for now).
- [ ] Note-on: resets EG to attack segment for all ops, captures velocity,
      sets `voice_rand`.
- [ ] Note-off: gates all op EGs to release.
- [ ] Per-op pan: voice produces mono in this ticket; a separate stereo
      summing pass (`voice_tick_stereo`) consumes the per-op pan parameters
      and produces L/R from the per-op outputs. Both paths exposed; stereo
      is the default once 0005 lands.
- [ ] Voice produces zero output when all op EGs are at L4=0 in release
      (steady-state idle). Used by the allocator (0004) to free voices.
- [ ] Bench: `voice_steady` (sustained note, all 6 ops active) and
      `voice_release` (note-off tail) added to `vxn2-osc-bench`.

## Notes

The voice's `pitch` is `note + bend + glide + master_tune`. Bend and master
tune apply at the voice level, not at the op (which scales pitch by per-op
ratio).

Stack-instance metadata fields go in the Voice struct now so that 0005 just
populates them at allocation time without refactoring the tick path. With
density 1 they default to `voice_idx=0, voice_spread=0, voice_rand=fixed_rng`.

Don't optimise the per-voice loop with rayon or threads — voices are SIMD-
parallelised across lanes in the next stacking pass (0005). Per-voice
threading would lose more to scheduling overhead than it gains.
