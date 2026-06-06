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

- [x] At `note_on`, the allocator (0004) allocates `stack_density` voices,
      not one. Each gets:
      - `voice_idx` ∈ {0..N−1}
      - `voice_spread` ∈ {−1, …, +1} (symmetric across stack, 0 = centre)
      - `voice_rand` ∈ [0, 1) (drawn from an xorshift RNG seeded per
        note-on for reproducibility across re-renders)
- [x] Macro knobs map into per-instance offsets:
      - `stack_detune` × `voice_spread` = per-instance pitch offset (cents)
      - `stack_spread` × `voice_spread` = per-instance pan (±1)
      - `stack_phase` × `voice_rand` = per-instance Q32 phase offset
        applied to all 6 ops at note-on
- [x] `stack_distrib` controls how `voice_spread` distributes:
      - `Linear`: even spacing across [−1, +1]
      - `Geometric`: exponential clustering toward outer instances
      - `Random`: each instance picks a fresh `voice_spread` per note-on
- [x] SoA voice batch: process up to 8 stacked voices in lockstep using
      `[f32; 8]` lane packing. The hot path must vectorise (verified via asm
      dump on Apple Silicon NEON). No per-instance branches on tunable params.
- [x] At `note_off`, all stacked instances gate to release together.
- [x] At `note_steal`, the oldest stack's instances are all reclaimed (not
      individually).
- [x] Voice budget: with 16-note polyphony × 8 density = 128 op-voice
      instances simultaneously. Set a documented CPU budget in this ticket
      (target: < 60% of one Apple M1 P-core at 44.1 kHz / 64-sample block).
- [x] Bench: `stack_d1`, `stack_d4`, `stack_d8` for a sustained 4-note
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

## Implementation summary

- `vxn2-dsp::stack::Stack` owns the SoA hot-path state: per-op `phase[8]`,
  `phase_inc[8]`, `fb_prev1[8]`, `fb_prev2[8]`, `base_phase_inc[8]` plus
  shared scalars (EG, `fb_scale`, `amp_sens_coef`). Per-stack metadata:
  `voice_idx[8]`, `voice_spread[8]`, `voice_rand[8]`, pre-computed
  `pan_l[6][8]`/`pan_r[6][8]` (carrier mask + active-lane mask + equal-power
  pan curve baked in — no per-sample branch).
- `PolyAlloc` was refactored from `[Voice; 16]` to `[Stack; 16]` — one Stack
  per played note holds up to 8 lanes, satisfying the "16 × 8 = 128 op-voice
  instances" budget. Steal / note_off act on whole stacks.
- Lane-packed router `LaneRouteFn`: one specialised `#[inline(never)]` fn per
  algorithm; each per-edge accumulation is a 0..8 loop LLVM lowers to NEON
  `fadd.4s`/`fmul.4s` on Apple Silicon (verified in bench asm dump).
- `stack_distrib` evaluated once at note-on; `Random` mode draws from the
  same xorshift state, keeping per-render determinism (`stack_seed(note,
  velocity, counter)`).

## CPU budget — measured

Bench host: Apple M1 P-core, `--release` profile (LTO=thin, codegen-units=1).
Bench: `stack/stack_d{1,4,8}` — sustained 4-note chord, algo 5
(three 2-stacks, 3 carriers, 3 mod edges), per-op feedback=2.

| Density | Wall-clock per iter | Throughput      |
|---------|---------------------|-----------------|
| 1       | 53.6 µs             | 19.1 Melem/s    |
| 4       | 53.5 µs             | 76.5 Melem/s    |
| 8       | 55.4 µs             | 147.9 Melem/s   |

Iter = 4 stacks × 256 samples = 1024 stack-ticks. d1→d8 wall-clock is flat
(d8 is +3% vs d1) because inactive lanes are still computed branch-free —
the SIMD lane packing pays the cost regardless of density. Throughput
scales 8× from d1 to d8: the sub-linear-with-density goal is met.

Extrapolated to 44.1 kHz / 64-sample blocks with 16 stacks × density 8:

```text
per stack-tick: 55.4 µs / 1024 ≈ 54 ns
per block:      16 stacks × 64 samples × 54 ns ≈ 55 µs
block period:   64 / 44100 ≈ 1451 µs
CPU budget:     55 / 1451 ≈ 3.8% of one M1 P-core
```

Comfortably under the 60% target. Headroom remains for mod matrix (0008),
LFOs (0006), pitch/mod EGs (0007), delay (0010), and FDN reverb (0011)
before the kernel approaches the budget ceiling.

## asm verification

`fmla.4s` / `fmul.4s` / `fadd.4s` / `fcvtzs.4s` / `add.4s` / `ucvtf.4s`
appear in the inlined `stack_tick_stereo` hot path inside the bench binary
(target: `aarch64-apple-darwin`). Q-register paired loads/stores
(`ldp q0,q1,[…]` / `stp q0,q1,[…]`) confirm 128-bit lane traffic. The
8-lane `for k in 0..STACK_LANES` loops execute as 2× NEON-4 iterations.
No `b.eq` / `b.ne` branches inside the per-sample loop body — only the
unconditional structural ones.

x86 verified post-ticket (cross-compile `--target x86_64-apple-darwin`):

- SSE2 baseline (default Windows): packed `mulps` / `addps` / `andps` /
  `blendps` on `%xmm` (128-bit, 4-wide). 8-lane loops execute as 2
  iterations — identical code shape to NEON. ~72 packed ops in the
  bench-inlined hot path.
- AVX2 (`-C target-cpu=x86-64-v3`, modern Windows from ~2013): packed
  `vmulps` / `vaddps` on `%ymm` (256-bit, 8-wide). 8-lane loops execute
  as a single iteration — ~2× SSE2 throughput. ~752 packed ops in the
  bench hot path.

The shipping plugin can ship SSE2 baseline (works everywhere, NEON-
equivalent perf) and optionally a separate AVX2-targeted Windows binary
for free headroom on capable hosts. The SoA layout uses no platform
intrinsics — LLVM autovec lowers equivalently across all three targets.
