---
id: E041
product: monorepo
title: "Shared FX unification — one declick idiom, portable kernels across vxn-1 / vxn-1b / vxn-2 (behavioural, flagged re-baselines)"
status: closed
created: 2026-08-02
---

> **vxn-1 retired, 2026-08-27.** The original vxn-1 is archived under
> `archive/vxn-1/`, out of the workspace and not expected to compile.
> **vxn-1b is now the canonical virtual-analogue synth**, and it carries what
> was vxn-1's DSP: `vxn-dsp` moved to `vxn-1b/crates/vxn-dsp` with its name
> intact. Where this epic says "vxn-1" as an *adopter* of shared code, read
> **vxn-1b** — the kernels are the same ones. Where it names vxn-1's shells,
> engine or web port, that work is gone.

> **The behavioural epic.** *(Written 2026-08-02. Two of its premises have not
> survived contact: vxn-1 was archived 2026-08-27, so every "vxn-1 AND vxn-1b in
> one commit" rule is vacuous; and 0230's reverb mix-law change does not exist —
> see its close-out. Read the per-ticket close-outs for what actually shipped.)*
>
> Three enable/disable conventions exist today:
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
- [ ] **0230** — Reverb: bypass moves inside the kernel. (**The "linear mix
      law canonical / largest perceptual change" framing was wrong** — vxn-2
      has mixed equal-power since 5460922, 2026-06-22, six weeks before this
      epic; the claim came from stale doc comments. No mix-law change, no
      mid-mix dip, no compensation curve. See 0230's close-out.)
- [ ] **0231** — Delay: vxn-2-superset kernel + damping gate; vxn-1/1b adopt.
- [ ] **0232** — Limiter bypass wrapper + MasterFx/FxChain rewrites. (vxn-1's
      `MasterFx` was archived before this ran; what shipped is
      `Bypassable<StereoLimiter>` under both live synths and vxn-1b's chain.)

## Close-out (2026-09-01)

All five tickets closed; the three E040 prerequisites (0225-0227) closed
earlier. What the epic promised, and what actually happened:

- **Portable kernels.** `vxn-core-dsp` holds `StereoPhaser`, `StereoChorus`,
  `FdnReverb`, `StereoDelay` and `DynamicsBlock`, plus
  `Bypassable<StereoLimiter>`; `vxn-dsp` and `vxn2-dsp` are re-export shims.
  Porting an effect between synths is now its `Params` mapping and nothing else.
- **One declick idiom.** `BypassXfade` no longer exists as a type anywhere in the
  repo, and neither engine holds an outer bypass fade: every enable is a
  `WetFade` inside the kernel, gated by `is_active()` at the call site. The
  double-fade ban is structural rather than a convention — there is no outer
  fade left to double with.
- **vxn-2 unchanged.** Render hash `0x95ac9a59d27aaddd` across the whole epic.
- **vxn-1b re-baselined once**, in `0f41bca`, for the delay adoption — the only
  ticket here that changed what the ear gets. Listened in Reaper first.

Two premises did not survive contact, both recorded in their tickets: 0230's
"largest perceptual change" mix-law framing (vxn-2 had been equal-power for six
weeks when the epic was written) and 0231's "equal-power → linear mix" plus its
feedback-cap change (unreachable from the param range). Both came from doc
comments that had gone stale, which is worth remembering the next time an epic
is planned by reading them.

Two bugs fell out of consuming `WetFade` properly for the first time, both in
the same place: the active latch. 0231 found it written pre-tick, so an owner
that gates on `is_active` and stops ticking never saw the next `RisingClear`;
0232 found the same failure re-introduced by a block fast path that skipped the
tick. Both fixed and pinned.

One finding is deferred: [0344](../../tickets/open/0344-smoothed-glide-never-lands.md)
— a `Smoothed` glide never lands on a target near 1.0, so an effect toggled on
mid-render sits ~-97 dBFS under its patch mix. The fix re-baselines both synths,
which this epic's rules forbid, so it is its own ticket.

## Acceptance

- All FX portable: constructing a synth's chain from shared kernels requires
  only its `Params` mapping (demonstrated by the three migrated chains).
- vxn-2 hash unchanged across the whole epic; vxn-1/vxn-1b goldens re-captured
  only in `REBASELINE:` commits; declick d4 suite green against new baselines.
- Idle CPU unchanged: settled-off kernels take the bit-exact passthrough /
  true-skip path (busy_profile + idle profile vs [[vxn1-render-loop-optimized]]).
