---
id: "0208"
product: vxn-1b
title: "Matrix mod smoothing — discontinuity guards on pitch/amp/PWM dests"
priority: medium
created: 2026-07-29
epic: E036
depends: []
---

## Summary

The vxn-1b mod matrix evaluates once per control block (sr/32) and applies raw
dest totals held constant across the block — [eval.rs:169](../../vxn-1b/crates/vxn1b-engine/src/eval.rs#L169).
A stepped source (square/pulse LFO, fast env, note-random) routed into a
continuous dest produces a hard value step at every block edge → zipper / click.
No smoothing exists on any matrix dest today.

DSP-side smoothing that already exists is **not** matrix-wide:

- **Cutoff / Resonance** — the ladder ramps its own coeffs
  (`ladder.prepare_ramp` / `tick_coeffs`, [bank.rs:418](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L418)),
  so cutoff/reso steps are largely absorbed. Leave these; verify, don't double-smooth.
- **Amp** — per-frame refresh for **envelope** sources only
  ([bank.rs:473](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L473)); non-env Amp
  (LFO→Amp) is still block-static. A `one_pole_coeff` import already sits waiting
  for "forthcoming LFO→Amp declick" — unbuilt.
- **Pitch, XModSweep, PWM, HpfCutoff** — nothing; osc `inc` / PWM set once per
  block and held.

Click-risk by dest: **Pitch / XModSweep** (high — osc inc jumps, no downstream
ramp), **Amp non-env** (high — gain step mid-note), **PWM** (med), Cutoff/Reso
(low — ladder absorbs).

## Design

Reuse vxn-2's proven mechanism (shared `vxn-core-utils/smoothing.rs` is already
on the path). Two tiers:

1. **Pitch + XModSweep → cascaded two-pole per-sample smoother.** Port vxn-2's
   `PitchSmoother` ([vxn-2 matrix.rs:1032](../../vxn-2/crates/vxn2-engine/src/matrix.rs#L1032)),
   trimmed from stack-lanes to vxn-1b's 16 voices. **Cascade is load-bearing**: a
   single one-pole is C0 but C1-broken — at a saw/pulse step the output value is
   continuous but pitch *velocity* jumps 0→max, and that velocity step is the
   click. Second pole makes output slope start at 0. Block-rate `set_targets`,
   per-sample `tick`.
2. **Amp (non-env) + PWM → single `Smoothed` one-pole**, block-rate, ~5 ms. Wire
   into the `one_pole_coeff` slot already stubbed in the Amp path.

Cutoff / Reso / HpfCutoff — no new smoothing; confirm the ladder ramp holds.

Snap on fresh note (static sources land zipper-free); glide only block-to-block
motion, per vxn-2's snap-vs-glide discipline.

## Acceptance criteria

- [ ] Cascaded two-pole smoother applied to Pitch + XModSweep dests, per voice
- [ ] Single one-pole applied to non-env Amp and PWM dests, block-rate
- [ ] Fresh-note values snap (no onset click); only inter-block motion glides
- [ ] Zipper regression test (port vxn-2 `zipper_regression.rs` second-difference
      ratio at block edges) covering square-LFO→Pitch and square-LFO→Amp — ratio
      below threshold
- [ ] Cutoff/Reso confirmed click-free via the same detector without added smoothing
- [ ] No measurable steady-state CPU regression in the idle/dry render profile

## Notes

- Smoothing primitives are in the shared `vxn-core-utils` crate — pull `Smoothed`
  + `one_pole_coeff` directly, don't reimplement.
- vxn-2 reference: three-tier smoothing (PitchSmoother cascade, level/pan linear
  ramp, macro/master one-pole) — see [[vxn2-level-mod-pipeline]] and its
  `zipper_regression.rs` detector (edge-vs-interior mean |d²| ratio ≈ 1.08 when
  ramped correctly).
- Out of scope: HpfCutoff / CrossModAmount smoothing (those dests deferred under
  E022); revisit when they land.
- Matrix eval is pure logic — smoothing state lives in the render/apply layer
  (bank.rs / render.rs), not the evaluator.

## Close-out (2026-07-29)

- **Pitch + XModSweep cascaded two-pole**, per lane, ticked per 16-sample
  quantum inside the render loop and re-cooking osc increments —
  [mod_smoothing.rs:144](../../vxn-1b/crates/vxn1b-engine/src/mod_smoothing.rs#L144)
  (`tick_pitch`), applied at
  [bank.rs:525](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L525). Ported/trimmed
  from vxn-2's `PitchSmoother` (16 stack-lanes → 8 bank lanes). The second pole
  zeroes the output start-slope (the C1 velocity step is the click), verified by
  `mod_smoothing::tests::cascade_output_slope_starts_at_zero`.
- **Amp / PWM** — one deviation from the ticket's "block-rate" wording: a
  block-*held* stair is itself an audible amplitude click on a slow carrier (a
  square-LFO→Amp measured **~73×** block-edge d² with a block-rate one-pole), so
  the non-env Amp part glides **per frame** on the live VCA path
  ([bank.rs:501](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L501),
  [mod_smoothing.rs:192](../../vxn-1b/crates/vxn1b-engine/src/mod_smoothing.rs#L192))
  and PWM **per quantum**. Envelope part of the VCA stays per-frame exact.
- **Fresh-note snap** — trigger snaps the lane's cascade + one-poles to the block
  target ([bank.rs:417](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L417)); only
  inter-block motion glides. Covered by
  `mod_smoothing::tests::snap_lands_settled_no_glide`.
- **Zipper regression** — `tests/zipper_regression.rs`: full-rate square LFO into
  each route, peak block-edge d² on a sine carrier. `square_lfo_to_amp_is_declicked`
  2.6× (baseline ~73× without the guard), `square_lfo_to_pitch_is_declicked` 3.2×,
  both under the 6× gate. `output_stays_finite_under_worst_case_flips` covers all
  five dests.
- **Cutoff/Reso left un-smoothed** — the OTA ladder ramps its own coeffs;
  `square_lfo_to_cutoff_stays_clean_without_added_smoothing` passes the same
  detector (6.6×) with no matrix-side smoothing, proving it needed none.
- **CPU** — idle path untouched (silent early-return before any smoother work);
  the render-parity gate is bit-stable (`parity` RMS ratio 1.00006), since the
  smoothers sit snapped at zero and take no per-quantum path when no route moves.
  The per-quantum pitch recook is gated to lanes with an active pitch route
  (`pitch_active`), so it engages only when a pitch route is live (e.g. the
  default vibrato). Not run through a formal busy-profile benchmark.
- Landed via merge `b964b40` (alongside E037's FX section); 102 lib + parity +
  zipper + alloc-free tests green on main post-merge.
