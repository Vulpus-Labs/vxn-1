---
id: "0242"
product: vxn-1b
title: "Cross-mod on the faceplate + live CrossModAmount matrix dest"
priority: high
created: 2026-08-04
epic: E039
depends: ["0219", "0208"]
---

## Summary

VXN1b can hard-sync, phase-modulate ("FM") and ring-modulate its oscillator
pair — the params exist (`cross_mod_type`, `cross_mod_amount`), the DSP kernels
exist (`process_sync` / `process_pm` / `poly_ring_mod`), and the mod matrix
offers **Cross-Mod Amt** as a destination. None of it is reachable or alive from
the player's side:

1. **No faceplate control.** [[0209]] removed VXN1's Cross Mod panel along with
   the five fixed-routing panels, on the reasoning that routing moved into the
   matrix overlay. That over-reached: cross-mod *type* and *amount* are **patch
   topology** (how the two oscillators are wired together), not a modulation
   depth. With the panel gone the only way to reach them is host automation, so
   sync / FM / ring are effectively missing from the instrument.
2. **The `CrossModAmount` dest is inert.** [[0202]] built the evaluator with the
   dest in `DEST_NAMES` / `DEST_LABELS` / `DEST_GAIN` (gain 4.0) and [[0208]]
   explicitly deferred its per-voice application ("out of scope: HpfCutoff /
   CrossModAmount smoothing"). `render::voice_cross_mod_amount` exists, is
   tested, and is called by nobody: `bank.rs` feeds `ctx.pm_index` — the raw
   patch scalar — to `process_pm`. Selecting Cross-Mod Amt in the matrix
   overlay today does nothing at all.

This ticket restores the panel and makes the dest live, per-voice and smoothed.

## Design

**Faceplate.** Port VXN1's Cross Mod panel into the layer pane's bottom row
(next to Voice — row 1 is full at four panels, row 3 has the space and the
`--panel-h-3` height): a `buttongroup` on `cross_mod_type` (Off/Sync/FM/Ring)
plus a fader on `cross_mod_amount` carrying `data-dim-unless-fm="cross_mod_type"`
so Amount greys out in the three modes that ignore it. Both cells are
`data-layered` — cross-mod is per-layer patch state. No new JS: the
buttongroup, fader and `unless-fm` dim rule are all already in the forked
dispatch/panel code.

**Per-voice PM index.** `PolyOscillator::process_pm` takes a scalar `pm_index`.
Generalise it over a `PmIndex` trait implemented for `f32` (broadcast) and
`&[f32; N]` (per lane), monomorphised like the existing `WaveKind` markers, so
VXN1's call site keeps its single-register load and the lane loop stays
branch-free ([[vxn1-soa-match-defeats-simd]]).

**Smoothing.** Cross-mod index is timbral, not amplitude: give it the PWM tier —
a per-lane one-pole ticked every `PITCH_QUANTUM` samples inside the render loop
(`MotionSmoother::tick_xmod`), snapped on note-on with the other slow one-poles.
Only the **matrix offset** is smoothed; the patch scalar is added on top, so a
patch with no route on the dest keeps `ctx.pm_index` exactly and stays on the
scalar kernel — render parity with VXN1 is untouched.

**Gating.** The dest applies in PM mode only (Sync/Ring/Off ignore amount, as
VXN1 does). The PM branch and the `osc1_runs`/`osc2_runs` engagement flags must
consider a modulated-from-zero index, or a patch with `amount = 0` plus an
Env→Cross-Mod Amt route would render silently unmodulated.

## Acceptance criteria

- [ ] Layer tabs show a Cross Mod panel: Type (Off/Sync/FM/Ring) + Amt, bound to
      the layer's `cross_mod_type` / `cross_mod_amount`, rebinding on layer flip.
- [ ] Amt dims unless Type reads FM; the CSS/mount contract tests still pass.
- [ ] A matrix slot into **Cross-Mod Amt** measurably changes the rendered
      spectrum in FM mode (engine test), and does nothing in Off/Sync/Ring.
- [ ] `amount = 0` + an active route into Cross-Mod Amt still produces FM.
- [ ] The offset is smoothed per quantum; a stepped source into Cross-Mod Amt
      does not raise the `zipper_regression` edge/interior ratio.
- [ ] No route on the dest ⇒ bit-identical render to before (parity test green).
- [ ] `tests/alloc_free.rs` green; VXN1's own tests green (shared `vxn-dsp` edit).

## Notes

- Closes the [[0208]] deferral for `CrossModAmount` only. `HpfCutoff` stays
  deferred — the HPF is set bank-wide (`set_cutoff_all`), so per-voice HPF is a
  separate kernel change.
- ADR 0001 §7 lists Cross Mod among the removed panels; amend it there so the
  document records the distinction (topology stays on the panel, routing depths
  move to the overlay) rather than contradicting the shipped faceplate.
- `render::voice_cross_mod_amount` stays the spec/tested statement of the
  clamp; `bank.rs` inlines the smoothed form, matching how PWM already works
  (`render::voice_pw` vs the inlined `(ctx.osc1_pw + pwm_s).clamp(..)`).

## Close-out (2026-08-06)

- **Panel restored.** CROSS MOD in the layer pane's bottom row —
  [faceplate.html:223](../../vxn-1b/crates/vxn1b-ui-web/assets/faceplate.html#L223):
  `buttongroup` on `cross_mod_type`, fader on `cross_mod_amount` with
  `data-dim-unless-fm`, panel `data-layered` so a tab flip rebinds both cells.
  Row share in [faceplate.css:252](../../vxn-1b/crates/vxn1b-ui-web/assets/faceplate.css#L252).
  No JS written: the buttongroup, fader and `unless-fm` dim rule were already in
  the forked dispatch. Covered by `tests::faceplate_mounts_resolve_to_real_params`
  and `tests::css_covers_every_control_primitive`.
- **Dim rule.** `__tests__/cross-mod-dim.test.js` (2 tests): the rule resolves to
  the *active layer's* `cross_mod_type` id (upper and lower), and its predicate
  dims Off/Sync/Ring, not FM.
- **Per-lane PM index.** `process_pm` is generic over a new `PmIndex` trait —
  `f32` broadcast or `&[f32; N]` per lane, monomorphised like `WaveKind` so the
  lane loop stays branch-free —
  [oscillator.rs:177](../../vxn-1/crates/vxn-dsp/src/poly/oscillator.rs#L177).
  VXN1's call site is unchanged (`vxn-dsp` 92 tests, `vxn-engine` 142 tests green).
- **Dest wired.** [bank.rs](../../vxn-1b/crates/vxn1b-engine/src/bank.rs) reads
  `DestId::CrossModAmount` per lane, gated to PM mode, and feeds the per-lane
  kernel only when a route is live. `tests/cross_mod_dest.rs`:
  `route_equals_the_same_amount_dialled_into_the_patch` (velocity route ≡ the
  same index dialled in, max diff < 1e-5),
  `route_drives_fm_from_a_zero_patch_amount`, `dest_is_inert_outside_fm_mode`,
  `negative_total_clamps_to_zero_index`, `route_is_per_voice_not_per_bank`.
- **Smoothing.** PWM-tier one-pole on the matrix *offset* only, ticked per
  `PITCH_QUANTUM`, snapped on note-on —
  [mod_smoothing.rs:200](../../vxn-1b/crates/vxn1b-engine/src/mod_smoothing.rs#L200),
  `mod_smoothing::tests::xmod_one_pole_glides_settles_and_snaps`.
  `zipper_regression::square_lfo_to_cross_mod_amount_is_declicked` measures
  3.13× edge/interior d² (bound 6.0), and the dest joins
  `output_stays_finite_under_worst_case_flips`.
- **Parity + RT.** `parity::default_patch_render_matches_vxn1` green — an
  unrouted patch keeps `ctx.pm_index` bit-exact on the broadcast kernel.
  `alloc_free::hot_path_is_allocation_free` green.
- **ADR 0001 §7 amended** to record why the panel came back: routing depths move
  to the overlay, oscillator *topology* stays on the faceplate.
- Verified at 6605617 in a clean worktree — the shared tree carries another
  session's in-flight 0244/0245 work. **Outstanding:** the DAW listening check
  (FM index under an env/LFO) is the user's, per [[verify-audio-in-reaper]].
