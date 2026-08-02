---
id: "0228"
product: monorepo
title: "Shared StereoPhaser (vxn-2 superset) — vxn-1 + vxn-1b adopt, outer fades deleted"
priority: medium
created: 2026-08-02
epic: E041
depends: ["0227"]
---

## Summary

First ticket of [E041](../../epics/open/E041-shared-fx-unification.md). vxn-2's
[phaser.rs](../../vxn-2/crates/vxn2-dsp/src/phaser.rs) is a strict superset of
vxn-1's ([phaser.rs](../../vxn-1/crates/vxn-dsp/src/phaser.rs)): same allpass
core, plus `PhaserParams` snapshot, `set_enabled` + `mix: Smoothed` +
`mix_primed`, `wet_makeup()`. Move the vxn-2 kernel to
`vxn-core-dsp::phaser` implementing `FxKernel`; vxn-1 and vxn-1b adopt it.

## Acceptance criteria

- [ ] Move commit: vxn-2 render hash unchanged (pure move for vxn-2).
- [ ] Adoption commit (vxn-1 AND vxn-1b together — parity oracle must not
      break in a window): vxn-1 maps its positional `set_params(rate, depth,
      fb, mix)` onto `PhaserParams` and deletes `phaser_fade`
      ([lib.rs:180-184](../../vxn-1/crates/vxn-engine/src/lib.rs#L180-L184));
      vxn-1b drops the PHASER slot fade in
      [fx.rs](../../vxn-1b/crates/vxn1b-engine/src/fx.rs). No kernel wrapped
      by both an internal WetFade and an outer fade (grep check).
- [ ] `REBASELINE:` commit only: vxn-1 phaser-toggle declick expectations +
      baseline where the patch engages phaser; vxn-1b zipper/d4. A/B rendered
      notes attached; user listens in Reaper first.

## Notes

vxn-2 pins 4 stages / CENTER_HZ 600 / spread/width/jitter; check vxn-1's
audible surface maps cleanly — any param vxn-1 exposes that the superset
lacks blocks this ticket (none expected per survey). [[verify-audio-in-reaper]]
