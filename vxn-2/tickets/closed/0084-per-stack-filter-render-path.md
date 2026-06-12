---
id: "0084"
title: "Per-stack filter state + stack-major oversampled render path, gated bypass"
priority: high
created: 2026-06-12
epic: E007
depends: ["0080", "0081", "0082", "0083"]
---

## Summary

Integration keystone of [E007](../../epics/open/E007-optional-per-voice-filter.md).
Wire the ported ladder ([0080](0080-port-ota-ladder-kernel.md)), resamplers
([0081](0081-port-halfband-decimator.md)/[0082](0082-halfband-interpolator.md)),
and params/dests ([0083](0083-filter-params-and-matrix-dests.md)) into the engine
as an *optional* per-voice oversampled filter sitting post-stack-sum /
pre-voice-sum, with a single shared decimation deferred past the voice-sum
(ADR 0004 §2–§5).

The per-sample control state needed to render a voice was verified
**already per-stack** (`pitch_smoothers[N_STACKS]`,
`level_mod_inc[stack][op][lane]`, all ramped into Stack-owned fields), so the
stack-major reorder this needs is cheap and requires no precompute-to-arrays.

## Design

Per-stack filter state: two `OtaLadderKernel`s (L/R) per stack — on the `Stack`
struct or a parallel `[(_, _); N_STACKS]` engine array. Plus one reusable
per-voice OS scratch (`[f32; MAX_BLOCK * 8]` ×2 stereo) and one OS bus
(`[f32; MAX_BLOCK * 8]` ×2), engine-owned, allocated once (not per block, not
when off).

Block-rate dispatch in `process_block` (`engine.rs`), **one** branch on
`filter-enable`:

- **OFF** — the existing sample-major loop, byte-for-byte unchanged
  (`for sample { for stack { stack_tick_stereo → dry += } } → fx`). No OS
  buffers touched, no per-sample branch added. This path must remain literally
  the current code.
- **ON** — stack-major:
  1. Per active stack: render its whole block to base-rate scratch via
     `stack_tick_stereo`, advancing that stack's pitch-smoother (every
     `PITCH_SMOOTH_QUANTUM`) and mod-ramps inside its own inner sample loop.
  2. `interpolate` scratch → `os_scratch` at factor F.
  3. Per oversampled sample: `kernel_l.tick` / `kernel_r.tick`.
  4. Accumulate `os_bus += os_scratch` (voice-sum at F× rate).
  5. After all stacks: `decimate(os_bus) → dry` once (shared).
  6. `fx(dry)` exactly as today.

Coefficients: recompute `OtaLadderCoeffs` per block from `filter-cutoff` +
matrix `Cutoff` (log domain) and `filter-resonance` + matrix `Resonance`
(per-stack scalar, lane-0), at the **oversampled** sample rate (so `compute_g`'s
fs-dependent pole detune is correct). `set_response` from `filter-mode` /
`filter-slope`. Frozen for the block (no per-sample coeff ramp in v1).

On note-on for a stack, `reset()` its L/R kernels so a re-used stack slot starts
from clean filter state.

## Acceptance criteria

- [x] `filter-enable` off ⇒ render path is the current sample-major loop (kept
  literally as the `else` branch) and output is unaffected by every other filter
  param (`filter_off_ignores_filter_params` asserts bit-equality vs a default
  engine). OS buffers allocated once at `Engine::new`, never per block.
- [x] `filter-enable` on ⇒ LP/HP/BP/Notch × 2/4-pole each render finite,
  non-trivial output (`filter_on_all_modes_render`); resonance = 1 self-osc is
  bounded + finite at every F ∈ {1,2,4,8} (`filter_on_self_osc_is_bounded`);
  low-cutoff LP attenuates vs open at every F (`filter_on_lowpass_attenuates`).
- [x] Matrix `Cutoff` modulates the per-voice filter end-to-end
  (`matrix_cutoff_modulates_filter`: mod-wheel→Cutoff opens the filter, more
  energy). Cutoff applied in log/octave domain (`base · 2^octaves`, clamped).
- [x] Deferred decimation numerically equivalent to per-voice: the decimator is
  linear, validated directly (`decimate_is_linear_over_voice_sum`, vxn2-dsp) —
  the property ADR §4 relies on. (Engine-level superposition isn't testable:
  per-allocation random phase decorrelation makes a note non-reproducible across
  slots.)
- [x] Stack-major reorder is per-voice-correct: each stack advances its *own*
  pitch-smoother / mod-ramp inside its block loop via single-stack helpers;
  per-stack state is independent so the result matches the OFF interleave.
- [x] No RT allocations in `process_block` on either path (buffers + kernels +
  resamplers all pre-allocated); no `unwrap`/`expect`/panic; coeffs recomputed
  at block rate (`OtaLadderCoeffs::new` once per active stack per block).
  Odd / non-pow2 block lengths handled (`filtered_render_handles_odd_block_len`).

## Notes

Quiescence-skip is **not** in this ticket — every active stack is filtered every
block here; [0085](0085-quiescence-skip.md) adds the skip on top. Latency
reporting is [0086](0086-latency-reporting.md). Keep the ON-path render body
factored so 0085 can insert the per-stack skip check cleanly and 0086 can read
the resampler group delay.
