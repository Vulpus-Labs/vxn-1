---
id: "0277"
product: vxn-1
title: "vxn-dsp: phaser stereo spread + delay feedback crossfeed toggle (kernel-level, behaviour-preserving defaults)"
priority: medium
created: 2026-08-21
epic: null
depends: []
---

## Summary

Both shared FX kernels hardcode their stereo behaviour, so neither VXN1 nor
VXN1b can offer it as a control:

- [phaser.rs](../../vxn-1/crates/vxn-dsp/src/phaser.rs) pins `SPREAD = 1.0` and
  reads the right cascade's LFO at a fixed `+0.5` cycle offset
  ([phaser.rs:275](../../vxn-1/crates/vxn-dsp/src/phaser.rs#L275),
  [phaser.rs:317](../../vxn-1/crates/vxn-dsp/src/phaser.rs#L317)). Anti-phase is
  the only sweep the kernel can do.
- [delay.rs](../../vxn-1/crates/vxn-dsp/src/delay.rs) *always* cross-feeds:
  `self.left.write(in_l + fb_r)`
  ([delay.rs:171-172](../../vxn-1/crates/vxn-dsp/src/delay.rs#L171-L172)). There
  is no straight (per-channel) feedback mode.

This ticket is the kernel half only — new arguments with defaults that keep the
existing render bit-identical. The param tables and faceplates land in
[0278](0278-vxn1-fx-stereo-params.md) (VXN1) and
[0279](0279-vxn1b-fx-stereo-params.md) (VXN1b).

## Design

**Phaser — `spread`.** `set_params` gains a `spread: f32` in `[0, 1]`, carried
on the struct and used as `tick_offset(0.5 * spread)`. `spread = 1.0` is
today's anti-phase sweep (the default, so nothing moves); `0.0` sweeps both
cascades in lockstep. Note the two channels keep their decorrelated
`stage_ratio` scatter (different seeds), so `spread = 0` is *near*-mono, not
L == R — the test asserts correlation, not equality. Update the module docs:
`SPREAD` stops being a pinned constant and joins the macro surface.

**Delay — `crossfeed`.** `set_params` gains `crossfeed: bool` (default `true` =
today). Off writes `in_l + fb_l` / `in_r + fb_r`, so each line's feedback stays
on its own side. Input routing is *not* changed — the toggle is crossfeed-only,
matching VXN2's `pingpong` flag semantics minus its input swap
([vxn2 delay.rs:274-280](../../vxn-2/crates/vxn2-dsp/src/delay.rs#L274-L280)).

Both signatures grow an argument, so every in-repo call site updates in the same
commit: [vxn-engine lib.rs](../../vxn-1/crates/vxn-engine/src/lib.rs),
[vxn1b fx.rs](../../vxn-1b/crates/vxn1b-engine/src/fx.rs),
[vxn-wasm bench.rs](../../vxn-1/crates/vxn-wasm/src/bench.rs).

## Acceptance criteria

- [ ] `StereoPhaser::set_params` takes `spread`; `spread = 1.0` reproduces the
      pre-change output sample-for-sample (existing `block_matches_per_sample`,
      `stereo_decorrelates_on_mono_input` etc. pass unmodified).
- [ ] New test: mono input at `spread = 0.0` yields a materially higher L/R
      correlation than at `spread = 1.0`.
- [ ] `StereoDelay::set_params` takes `crossfeed`; `crossfeed = true` is
      bit-identical to today (existing delay tests pass unmodified).
- [ ] New test: with `crossfeed = false` and an L-only impulse, R output stays
      exact zero over several delay periods; with `crossfeed = true` it does not.
- [ ] All call sites updated; `cargo test -p vxn-dsp -p vxn-engine -p
      vxn1b-engine` green, incl. the VXN1b render-parity oracle
      ([parity.rs](../../vxn-1b/crates/vxn1b-engine/tests/parity.rs)).

## Notes

- Defaults are chosen so no preset, golden, or parity test re-baselines here.
  The audible change arrives only when a user moves the new controls (0278/0279).
- Interacts with [E041](../../epics/open/E041-shared-fx-unification.md): the
  shared-kernel superset in **0228** (phaser) and **0231** (delay) must include
  `spread` and `crossfeed`, since VXN2's kernels are the canon there and VXN2's
  phaser has no spread control yet ([0280](0280-vxn2-fx-stereo-params.md) adds
  it). Doing this now keeps the two kernels converging rather than diverging.
- Out of scope: true ping-pong (input summed to one side) — the toggle is
  crossfeed-only, per the product decision.


## Close-out (2026-08-21)

- `StereoPhaser::set_params` takes `spread` in `[0, 1]`, carried on the struct
  and read as `tick_offset(0.5 * spread)` on both the per-sample and block
  paths ([phaser.rs:277](../../vxn-1/crates/vxn-dsp/src/phaser.rs#L277)). Module
  docs updated — `SPREAD` left the pinned-constants list and joined the macro
  surface.
- `StereoDelay::set_params` takes `crossfeed: bool`; `false` writes
  `in_l + fb_l` / `in_r + fb_r` instead of the crossed pair
  ([delay.rs:131](../../vxn-1/crates/vxn-dsp/src/delay.rs#L131)). Input routing
  unchanged in both modes, per the crossfeed-only decision.
- Defaults hold the old behaviour (`spread = 1.0`, `crossfeed = true`), proven
  by the untouched goldens: vxn-1's `baseline_render_is_stable` and vxn-1b's
  `default_patch_render_matches_vxn1` / `default_patch_amp_env_matches_shape`
  both pass unmodified.
- New: `phaser::tests::spread_zero_recorrelates_channels` (mono-source L/R
  correlation > 0.95 at spread 0, and > wide + 0.05) and
  `delay::tests::crossfeed_off_keeps_feedback_on_its_own_side` (L-only impulse
  leaves R at exact zero with crossfeed off, non-zero with it on). Every
  existing phaser/delay test kept its assertions — only the new argument was
  added at the call.
- All three in-repo call sites updated (`vxn-engine`, `vxn1b-engine`,
  and the phaser/delay test bodies); `cargo test --workspace` green.
