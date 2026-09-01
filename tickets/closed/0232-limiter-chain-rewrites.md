---
id: "0232"
product: monorepo
title: "Bypassable<StereoLimiter> + MasterFx / FxChain rewrites as FxKernel sequences with true-skip"
priority: medium
created: 2026-08-02
epic: E041
depends: ["0228", "0229", "0230", "0231"]
---

## Summary

Final ticket of [E041](../../epics/open/E041-shared-fx-unification.md).
`StereoLimiter` stays in vxn-core-utils; add a `Bypassable<StereoLimiter>`
wrapper in `vxn-core-dsp::fx` carrying WetFade + the off→on edge-reset glue
both engines duplicate (vxn-1
[lib.rs:300-306](../../archive/vxn-1/crates/vxn-engine/src/lib.rs#L300-L306); vxn-2
`limiter_was_on` in
[engine.rs:1062-1069](../../vxn-2/crates/vxn2-engine/src/engine.rs#L1062-L1069)).
Then rewrite both chains as thin `FxKernel` sequences:

- vxn-1 `MasterFx::process_block`: per-stage arm/clear/blend plumbing replaced
  by kernel calls with `is_active()` skip; `limiter_fade` (last remaining
  `BypassXfade` slot) deleted. vxn-1 gains the true-skip vxn-1b already has —
  correct now because settled-off passthrough is bit-exact by the FxKernel
  contract.
- vxn-1b `FxChain`: `fades`/`on` arrays and `retarget`/`blend` deleted; slots
  become shared kernels; per-sample serial loop can stay or go block-wise —
  whichever keeps the diff smallest.

## Acceptance criteria

- [ ] No `BypassXfade` used for any per-FX enable anywhere (grep); it remains
      only in whole-span sites (vxn-1 `OutputStage` OS change, vxn-2 span).
- [ ] Bit-exact-when-idle guarantee holds: engine output with all FX
      disabled+settled is byte-identical to an effect-absent build (existing
      declick.rs assertion, re-anchored).
- [ ] `REBASELINE:` limiter-toggle declick expectations.
- [ ] Idle/steady-state CPU unchanged: busy_profile + idle profile vs
      [[vxn1-render-loop-optimized]]; `master_chain` bench within noise.

## Notes

Chain order stays per-synth (vxn-1: phaser→chorus→delay→reverb→limiter;
vxn-1b: dynamics→chorus→phaser→delay→reverb) — order is voicing, not
plumbing. Dynamics already migrated in 0227; vxn-1b's DYNAMICS slot just
drops its outer fade here if 0227's WetFade commit didn't already.

## Close-out (2026-09-01)

Three commits: the shared wrapper (`cb8fdd6`), vxn-2's engine (`ba2da10`), then
vxn-1b's engine and chain (`0828a38`). Last ticket of E041.

### Scope, as it actually stood

vxn-1 was archived on 2026-08-27, so half this ticket — `MasterFx`, its
`limiter_fade`, its per-stage arm/clear/blend — was already gone. What remained
is what shipped: `Bypassable<StereoLimiter>` under both live synths, and vxn-1b's
`FxChain` rewritten as kernel calls.

### Acceptance

- **No `BypassXfade` for a per-FX enable.** Stronger than the criterion asked:
  the *type* no longer exists anywhere in the repo. `grep -rn BypassXfade`
  returns two hits, both prose — `declick.rs`'s account of what the idiom
  replaced, and a `smoothing.rs` note about a clamp that outlived it. The
  whole-span sites the ticket carved out (vxn-1's `OutputStage`, vxn-2's span)
  build on `raised_cosine_rise` directly and never used the type.
- **Bit-exact when idle.** `vxn1b-engine::fx::tests::all_off_is_bit_exact_passthrough`
  holds from the first sample (the chain's fades used to *start* snapped to 0;
  now there are no fades to snap). vxn-2's render hash is unchanged at
  `0x95ac9a59d27aaddd`, and its null test passes.
- **`REBASELINE:` limiter-toggle expectations — not needed, and that is a
  result, not an omission.** Both baseline patches have the limiter off, so no
  golden moved. vxn-1b's `oversampling_limiter::engaging_the_limiter_does_not_click`
  passes unchanged: it measures the join step against the signal's own motion,
  and the fade it measures is the same 10 ms one, just held somewhere else.
  vxn-2 *gains* that fade (see below) and had no test asserting the step it used
  to make.
- **CPU.** `master_chain` full: 570 µs before, 631 µs after, but `master_chain_fx_off`
  read 571 µs before and 559 µs after on the same pair of runs — the spread
  across runs is wider than the difference, so this bench cannot resolve the
  change. What actually changed on that path, with the limiter off, is a
  `set_enabled` and one early return per block. The vxn-1b side is strictly less
  work than before: five slots lost an array index, a `Smoothed::tick` and a
  crossfade each.

### vxn-2's limiter toggle now fades

The one behaviour change here. vxn-2 stepped dry→limited, which jumps the level
by whatever gain reduction was active; it now crossfades over 10 ms, the window
vxn-1b already used. Fully engaged is still bitwise the bare limiter — the
wrapper's block path hands the block over untouched — and bypassed-and-settled
is still a true skip.

### The trap in the block fast path

`Bypassable::process_block` skips per-sample work when the fade is settled full,
and the obvious version skipped the `tick` with it. The weight is 1.0 either
way, but `WetFade`'s active latch and its rising edge live in `tick`: skipping
left the latch reading "inactive", so the first sample that later fell to the
per-sample path reported a `RisingClear` and wiped the running limiter
mid-block. One tick per block fixes it (a settled smoother's tick is
idempotent). Caught by `limiter::tests::block_and_sample_paths_agree_across_a_fade`
before it left the crate — this is the second latch bug in E041's last two
tickets, both from an owner that stops ticking.

### What this uncovered: 0344

Wanting an exact full-wet weight exposed that **a `Smoothed` glide never lands
on a target near 1.0**: the snap threshold is an absolute 1e-6, while the
one-pole's increment drops below half a ULP with ~1.4e-5 still to go. So a fade
ridden *up* to full sits ~-97 dBFS short of the mix its patch asked for, forever.
The fix was written, measured — vxn-2's null test moves to -79.55 dBFS, its hash
to `0x3666245aa155a378` — and reverted, because E041 requires vxn-2's render to
be unchanged by its migrations. Filed as [0344](0344-smoothed-glide-never-lands.md).

Consequence carried into the code: `WetFade::settled_full` is reachable by a
snap (patch load, first `set`, reset) and not by a glide, so the block fast path
applies to a limiter that loaded engaged and not to one toggled on mid-render.
Documented on the method rather than left as a surprise.

### Left standing

`DynamicsBlock` still hand-rolls the fade `WetFade` was extracted from — same
fields, same rules, spelled twice. It gained a `reset` here (0230's un-prime
lesson, which it had never learned) but not a `WetFade`, because swapping it is a
bit-exactness question for vxn-2's render rather than a chain-rewrite detail.
