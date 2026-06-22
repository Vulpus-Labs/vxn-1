---
id: "0089"
product: vxn-2
title: "Wire phaser into engine bus (pre-delay)"
priority: medium
created: 2026-06-22
epic: E025
depends: ["0088"]
---

## Summary

Third ticket of [E025](../../epics/open/E025-vxn2-fx-tabs-phaser.md).
Insert the ported `StereoPhaser` into the engine FX bus pre-delay,
fan params through `apply_block_params()`, and clear state on reset.

## Design

Engine: `vxn-2/crates/vxn2-engine/src/engine.rs`.

- Add `pub phaser: StereoPhaser` to the `Engine` struct (near
  `delay` / `reverb`, lines 164–165); construct in `Engine::new`.
- Bus (off-path sample loop, lines 1104–1106): insert phaser
  **pre-delay** so the chain reads:
  ```
  let (cl, cr) = self.cleanup.process(dry_l, dry_r);
  let (l, r) = self.phaser.process(cl, cr);   // new
  let (l, r) = self.delay.process(l, r);
  let (l, r) = self.reverb.process(l, r);
  ```
- `apply_block_params()` (line 462): add
  `self.phaser.set_params(&self.params.phaser, ...)` — gate on
  `phaser-on`; when off, either bypass `process` or feed mix=0 such
  that output is bit-identical to the pre-epic bus.
- `Synth::reset`: call `self.phaser.clear()` alongside the existing
  delay/reverb tail clears.

`phaser-on = 0` must be a deterministic null vs. pre-epic output —
mirror how `delay-on` / `reverb-on` gate their stages.

## Acceptance criteria

- [ ] Phaser constructed in `Engine`, inserted pre-delay in the bus.
- [ ] `apply_block_params()` fans `PhaserParams` to
      `phaser.set_params(...)`.
- [ ] `Synth::reset` clears phaser allpass state.
- [ ] With `phaser-on = 0`, rendered output is bit-identical to
      pre-epic (null test, deterministic seed).
- [ ] With `phaser-on = 1`, sweeping rate/depth/fb/mix audibly drives
      the DSP.
- [ ] `cargo test -p vxn2-engine` passes.

## Notes

Rate glide via `Smoothed` is held inside the phaser (0087); the engine
just pushes targets per block. Sample-rate changes re-init the phaser
like the other FX.

## Close-out (2026-06-22)

- `pub phaser: StereoPhaser` added to `Engine`, constructed in `Engine::new`,
  inserted **pre-delay** in both bus sample loops
  (`cleanup → phaser → delay → reverb`): the off-path (≈1106) and the
  oversampled on-path (≈1281).
- `apply_block_params()` fans `self.phaser.set_from(&self.params.phaser)`
  (sets the on-gate + the four floats). `Engine::reset` calls
  `self.phaser.clear()` alongside the delay/reverb tail clears.
- `phaser-on = 0` → bit-exact passthrough (the DSP `enabled` gate returns the
  input untouched), so a pre-epic patch renders bit-identical — covered by
  `disabled_is_bit_exact_passthrough` (0087) and the engine null suite.
- `phaser-on = 1` sweeps audibly: `param_audibility.rs` gained a phaser
  context-override (on + depth/mix/fb + long window); all four phaser faders
  register rel-diff > eps. `cargo test -p vxn2-engine` + `--workspace` pass.
