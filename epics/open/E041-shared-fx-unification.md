---
id: E041
product: monorepo
title: "Shared FX unification — one declick idiom, portable kernels across vxn-1 / vxn-1b / vxn-2 (behavioural, flagged re-baselines)"
status: open
created: 2026-08-02
---

> **vxn-1 retired, 2026-08-27.** The original vxn-1 is archived under
> `archive/vxn-1/`, out of the workspace and not expected to compile.
> **vxn-1b is now the canonical virtual-analogue synth**, and it carries what
> was vxn-1's DSP: `vxn-dsp` moved to `vxn-1b/crates/vxn-dsp` with its name
> intact. Where this epic says "vxn-1" as an *adopter* of shared code, read
> **vxn-1b** — the kernels are the same ones. Where it names vxn-1's shells,
> engine or web port, that work is gone.

> **The behavioural epic.** Three enable/disable conventions exist today:
> vxn-1's outer raised-cosine `BypassXfade` per stage, vxn-2's in-kernel
> `Smoothed` wet + `mix_primed` + bit-exact passthrough when settled, vxn-1b's
> per-slot linear fade with true skip. This epic unifies on the **vxn-2
> idiom** (locked decision) and moves phaser / chorus / reverb / delay /
> limiter-bypass into `vxn-core-dsp` as `FxKernel` implementations, so an
> effect ports between synths by constructing its shared `Params` struct.
> Output of vxn-1/vxn-1b changes audibly in known ways; every change lands in
> a flagged `REBASELINE:` commit with rendered A/B notes.

## Goal

When this epic closes:

- `StereoPhaser`, `StereoChorus` (true-stereo only), `FdnReverb`, `StereoDelay`
  (vxn-2-superset + optional feedback damping), and `Bypassable<StereoLimiter>`
  live in `vxn-core-dsp`, all implementing `FxKernel` with `WetFade` declick.
- vxn-1's `MasterFx` and vxn-1b's `FxChain` are thin sequences of `FxKernel`
  calls with `is_active()` true-skip; the five `BypassXfade` fields and the
  five slot fades are gone (double-fade ban).
- vxn-2's render hash is **unchanged** — its kernels are the canon; the moves
  are pure for vxn-2.

## Rules

- Each FX migrates vxn-1 AND vxn-1b in **one commit** — the vxn-1b parity
  oracle must never break in a window.
- Every kernel adoption deletes the corresponding outer fade in the same
  commit (no kernel is ever wrapped by both an internal `WetFade` and an outer
  fade — grep-level acceptance criterion).
- `REBASELINE:` commits contain only new goldens + A/B notes, never code.
- User listening checks in Reaper before each re-baseline lands
  ([[verify-audio-in-reaper]]).

## Planned tickets

0228–0231 independent after E040/0227; **0232 last** (chain rewrites assume
all kernels migrated).

- [ ] **0228** — Phaser: vxn-2 superset shared; vxn-1/1b adopt.
- [ ] **0229** — Chorus: true-stereo only; mono-sum `process` deleted.
- [ ] **0230** — Reverb: linear mix law canonical (largest perceptual change).
- [ ] **0231** — Delay: vxn-2-superset kernel + damping gate; vxn-1/1b adopt.
- [ ] **0232** — Limiter bypass wrapper + MasterFx/FxChain rewrites.

## Acceptance

- All FX portable: constructing a synth's chain from shared kernels requires
  only its `Params` mapping (demonstrated by the three migrated chains).
- vxn-2 hash unchanged across the whole epic; vxn-1/vxn-1b goldens re-captured
  only in `REBASELINE:` commits; declick d4 suite green against new baselines.
- Idle CPU unchanged: settled-off kernels take the bit-exact passthrough /
  true-skip path (busy_profile + idle profile vs [[vxn1-render-loop-optimized]]).
