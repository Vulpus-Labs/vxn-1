---
id: "0005"
title: Voice stacking (density / detune / spread / phase / distrib)
priority: high
created: 2026-06-05
epic: E001
---

## Summary

When a note plays, instantiate N concurrent stacked voices (density 1..8)
with per-instance detune, pan, and phase distributed across the stack. Each
instance gets its `voice_idx`, `voice_spread`, `voice_rand` populated so the
mod matrix (0008) can route them. This is the VXN2 differentiator: cheap
hypersaw-style supersaws without wavetable storage.

## Acceptance criteria

- [ ] At `note_on`, the allocator (0004) allocates `stack_density` voices,
      not one. Each gets:
      - `voice_idx` ∈ {0..N−1}
      - `voice_spread` ∈ {−1, …, +1} (symmetric across stack, 0 = centre)
      - `voice_rand` ∈ [0, 1) (drawn from an xorshift RNG seeded per
        note-on for reproducibility across re-renders)
- [ ] Macro knobs map into per-instance offsets:
      - `stack_detune` × `voice_spread` = per-instance pitch offset (cents)
      - `stack_spread` × `voice_spread` = per-instance pan (±1)
      - `stack_phase` × `voice_rand` = per-instance Q32 phase offset
        applied to all 6 ops at note-on
- [ ] `stack_distrib` controls how `voice_spread` distributes:
      - `Linear`: even spacing across [−1, +1]
      - `Geometric`: exponential clustering toward outer instances
      - `Random`: each instance picks a fresh `voice_spread` per note-on
- [ ] SoA voice batch: process up to 8 stacked voices in lockstep using
      `[f32; 8]` lane packing. The hot path must vectorise (verified via asm
      dump on Apple Silicon NEON). No per-instance branches on tunable params.
- [ ] At `note_off`, all stacked instances gate to release together.
- [ ] At `note_steal`, the oldest stack's instances are all reclaimed (not
      individually).
- [ ] Voice budget: with 16-note polyphony × 8 density = 128 op-voice
      instances simultaneously. Set a documented CPU budget in this ticket
      (target: < 60% of one Apple M1 P-core at 44.1 kHz / 64-sample block).
- [ ] Bench: `stack_d1`, `stack_d4`, `stack_d8` for a sustained 4-note
      chord. Test scales sub-linearly with density (lane packing benefit
      visible).

## Notes

The hot-path SIMD lesson from VXN1: a `match` on an enum inside the lane
loop defeats vectorisation (per memory: `vxn1-soa-match-defeats-simd`). All
per-stack parameter variations must be expressed as numeric offsets, not
enum branches. The `stack_distrib` selection happens *once* at note-on (when
populating `voice_spread`), not at sample rate.

Per ADR §3, the stacking macros also expose their offsets *through* the mod
matrix as `voice_idx` / `voice_spread` / `voice_rand` sources. The macro
knobs are equivalent to pre-wired matrix slots. Implementation choice: keep
the macro path direct (no matrix indirection on the hot path) and let
additional matrix slots layer additively on top.

xorshift RNG: a single u64 state seeded from `note × velocity ^ counter`
gives reproducible random across re-renders, which is essential for
deterministic offline rendering.
