---
id: "0074"
title: "Smooth level/pan matrix modulation — zipper noise on LFO routes"
priority: high
created: 2026-06-10
epic: E006
depends: []
---

## Summary

Thirteenth ticket of [E006](../../epics/open/E006-review-remediation.md).
Routing an LFO to `OpNLevel` or `OpNPan` through the matrix produces
audible zipper noise: both destinations are projected once per control
block and applied as block constants, so the amplitude jumps at every
block edge (~750 Hz step rate at 64-sample blocks; worse at larger
host block sizes). Pitch destinations got the same treatment fixed in
0063 (`PitchSmoother`); level and pan never did.

## Where the steps happen

- Level: `op_level_mod` is projected at block rate
  ([engine.rs](../../crates/vxn2-engine/src/engine.rs) matrix
  projection, AmpSens-gated since 0062) and read per sample as a
  constant in `stack_tick_stereo`
  ([stack.rs:727](../../crates/vxn2-dsp/src/stack.rs)) —
  `(lvl + lvl_mod[k]).clamp(0.0, 1.0)`, no interpolation.
- Pan: `refresh_pan_with_mod`
  ([stack.rs:517-538](../../crates/vxn2-dsp/src/stack.rs)) recomputes
  `pan_l`/`pan_r` once per block; the stereo fold
  (stack.rs:741-745) multiplies the stepped gains every sample.

## Design

Linear ramp across the block (chosen over extending the 0063 one-pole
quantum machinery and over per-sample one-poles):

- At the block-rate projection site, keep the previous block's value
  per lane; compute `delta = (new - prev) / block_len`.
- Per-sample loop adds the increment: one add per lane per sample,
  branch-free, constant per lane — vectorises with the existing
  NEON lane layout (no runtime match in the loop).
- Pan: ramp the **gains** `pan_l`/`pan_r` directly between the old and
  new equal-power points — no per-sample `sin_cos`. The chord between
  two nearby points on the equal-power curve is inaudibly different
  from the arc at LFO rates.
- Exact convergence at block end (no residual error, unlike one-pole),
  zero cost when the target hasn't moved (delta = 0).
- Apply the same ramp to the scalar `op_tick` reference path so the
  reference and SoA paths stay equivalent.

Why not the alternatives: the 0063 16-sample quantum still steps (just
finer) — fine for pitch where the smoother's one-pole shape matters,
but level/pan want exact linear tracking; per-sample one-poles cost
6 ops × 8 lanes × 3 values of state + multiply with no audible win.

## Acceptance criteria

- [ ] LFO1 → Op1Level (carrier) at depth max, block size 256: rendered
  output has no spectral lines at block-rate harmonics beyond noise
  floor (test: compare against a 64-sample-block render of the same
  patch — fingerprints converge instead of diverging).
- [ ] LFO1 → Op1Pan, same setup: no zipper (same test shape on L/R
  difference signal).
- [ ] Static patch (no level/pan routes): `master_chain` bench within
  noise of HEAD — the ramp adds ≤ 1 add/lane/sample.
- [ ] Scalar `op_tick` reference path matches `stack_tick_stereo`
  output under level/pan modulation (existing equivalence test
  extended).
- [ ] Manual listen: slow LFO (~0.5 Hz) and fast LFO (~8 Hz) on level
  and pan, block sizes 64 and 512 — smooth at both.

## Notes

Found by ear 2026-06-10 (post-review; the review's audibility sweep
0069 compares min/max fingerprints and would not catch stepping —
zipper changes spectrum, not presence). The fold stage reads
`prev_outs` with a 1-sample delay convention; ramping the pan gains in
place keeps that intact. `refresh_pan_with_mod`'s per-block
`sin_cos` cost is unchanged — it just becomes the ramp target instead
of the applied value.

## Close-out (2026-06-10)

Linear block ramp shipped, with one design deviation: the ramp state
(increments + live flags) lives on the **engine**, not the `Stack`, and
the advance runs in `Engine::process_block`'s sample loop rather than
inside `stack_tick_*`.

Why: the first cut (fields on `Stack`, advance at the tail of the tick
fns) regressed `stack` benches +26% and `master_chain` +34% — the extra
fields/code disturbed the tick's codegen far beyond the nominal
one-branch cost. Moving the state engine-side restored `stack_tick_*`
to its exact pre-0074 shape: stack benches measure identical to HEAD
(p > 0.05), `master_chain` lands at +1.3% with the per-block increment
computation included (epic budget ≤ 5% cumulative).

- Exact convergence at block end (increment recomputed from the actual
  current value each block — no drift accumulation); zero per-sample
  cost when no level/pan route moves (`any_ramp_live` short-circuit,
  asserted by test).
- Pan ramps the folded equal-power gains (chord, not arc) via the new
  `Stack::pan_targets()`; per-block `sin_cos` count unchanged.
- Fresh allocations snap (shared `mod_seq` generation tracking with the
  0063 pitch smoother) — no ramp-in from a stolen voice's state.
- Scalar-path AC inapplicable: `op_tick` / `voice.rs` has no level/pan
  mod input (same finding as 0062/0073).
- The 64-vs-256 spectral fingerprint AC was replaced by direct state
  assertions (per-block convergence on the dest accumulator + ramp
  liveness): an RMS-envelope comparison cannot fail under the old
  block-stepping code, so it guarded nothing. Manual listen at block
  64/512 with slow + fast LFO still pending (manual AC).
- Folded in (user request): `1/√density` is now cached on the stack at
  note-on (`inv_sqrt_density`) instead of being recomputed per sample
  in `stack_tick_mono` and per block in the pan refresh.
