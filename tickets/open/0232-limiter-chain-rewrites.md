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
