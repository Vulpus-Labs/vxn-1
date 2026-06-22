---
id: "0087"
product: vxn-2
title: "Port StereoPhaser into vxn2-dsp"
priority: medium
created: 2026-06-22
epic: E025
---

## Summary

First ticket of [E025](../../epics/open/E025-vxn2-fx-tabs-phaser.md).
Port vxn-1's `vxn-dsp::phaser::StereoPhaser` into `vxn2-dsp` as a new
`phaser` module. Self-contained DSP — no engine/param/UI wiring (that
follows in 0088–0090).

## Design

Source: `vxn-1/crates/vxn-dsp/src/phaser.rs` — `StereoPhaser` with
4 allpass stages per channel, anti-phase L/R sweep, 600 Hz centre,
signed feedback clamped ±0.9, collapsed Rate/Depth/FB/Mix surface.

Target: `vxn-2/crates/vxn2-dsp/src/phaser.rs`, registered as
`pub mod phaser;` in `vxn2-dsp/src/lib.rs` (slot near `op`/`reverb`).

Public API to preserve:

- `new(sample_rate: f32) -> Self`
- `set_params(&mut self, rate_hz, depth, feedback, mix)`
- `process(&mut self, in_l, in_r) -> (f32, f32)`
- `clear(&mut self)` (state reset)
- `process_block_stereo(...)` if vxn-2 wants a block path (optional —
  vxn-2's bus is per-sample, so `process` alone may suffice).

Substitute vxn-1's smoother for vxn-2's
`vxn2-dsp::smoother::Smoothed` where the rate glide is held. Keep all
pinned internals as named consts (stages=4, centre 600 Hz, spread,
feedback clamp) — these are not user params.

## Acceptance criteria

- [ ] `vxn-2/crates/vxn2-dsp/src/phaser.rs` exists; `pub mod phaser;`
      added to `vxn2-dsp/src/lib.rs`.
- [ ] Upstream phaser unit tests ported and passing under
      `cargo test -p vxn2-dsp`.
- [ ] `mix = 0` → `process` returns dry input unchanged (passthrough
      null check).
- [ ] No new dependencies added to `vxn2-dsp`.
- [ ] No engine/param/UI changes in this ticket.

## Notes

Straight lift — see [[vxn1-fx-dual-chain-internally]] (phaser already
holds separate L/R chain state, so no mono-path shortcut). The vxn-2
bus runs per-sample (`engine.rs:1104`), so the `process(l, r)` path is
the one the engine will call.
