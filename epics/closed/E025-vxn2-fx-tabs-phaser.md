---
id: E025
product: vxn-2
title: FX tab panel + phaser
status: closed
created: 2026-06-22
---

## Goal

Two coordinated changes to vxn-2's FX surface, mirroring vxn-1's
closed [E009](../closed/E009-fx-tabs-phaser-fdn-drift.md):

1. **Add a Phaser** as a third FX. Port `StereoPhaser` from
   `vxn-1/crates/vxn-dsp/src/phaser.rs` into `vxn2-dsp`. Macro
   surface: Rate, Depth, FB, Mix (+ header on/off). Insert into the
   engine bus pre-delay: `dry → phaser → delay → reverb → master`.
2. **Consolidate Delay / Reverb / Phaser into one tabbed `FX`
   panel** with a tab strip, replacing today's panel-per-effect
   layout. Port vxn-1's `wireFxTabs` idiom and its test.

Phaser params are **host-automation only** — they go in the flat
CLAP param table but are **not** added as mod-matrix destinations.

## Background

vxn-2's FX chain is `cleanup → delay → reverb → master`
(`vxn2-engine/src/engine.rs:1104`). Adding a phaser as a third
panel-per-effect block would crowd the faceplate; vxn-1 already hit
this at four effects and solved it with a left tab strip
(`wireFxTabs` in `vxn-ui-web/assets/panels.js`, guarded by
`fx-tabs.test.js`). vxn-2 borrows the proven idiom rather than
inventing one.

The phaser already exists in production form in vxn-1
(`vxn-dsp::phaser::StereoPhaser` — 4 allpass stages, anti-phase L/R
sweep, 600 Hz centre, signed feedback clamped ±0.9, collapsed
Rate/Depth/FB/Mix surface). Porting it to `vxn2-dsp` is a straight
lift — vxn-2's `Smoothed` (`vxn2-dsp/src/smoother.rs`) substitutes
for vxn-1's smoother where the rate glide is wanted.

Unlike vxn-1's E009 this epic does **not** touch reverb (vxn-2 *is*
the FDN source E009 ported from) and adds **no** drift knob — those
were vxn-1-only catch-ups. Scope here is strictly phaser + tabs.

## In scope

- New `vxn2-dsp::phaser::StereoPhaser` ported from vxn-1's
  `vxn-dsp::phaser`. Upstream tests ported. Host-rate only, same
  pinned internals (stages=4, centre 600 Hz, anti-phase spread,
  feedback clamp ±0.9).
- Param table growth: **append** `phaser-on`, `phaser-rate`,
  `phaser-depth`, `phaser-feedback`, `phaser-mix` at the **end** of
  the CLAP table (new ids 196–200, `N` 196 → 201). Appending — not
  inserting — keeps existing delay/reverb param ids stable so saved
  DAW automation/sessions survive. New `OFF_PHASER` section offset,
  `PhaserParams` struct, decode arm in `shared.rs`.
- Engine bus: insert phaser **pre-delay** so the chain is
  `cleanup → phaser → delay → reverb → master`. Wire through
  `apply_block_params()` → `phaser.set_params(...)`; clear the
  phaser allpass state in `Synth::reset`. `phaser-on = 0` must keep
  the bus sample-exact (gated like `delay-on`/`reverb-on`).
- Faceplate: replace the standalone `.delay-panel` + `.reverb-panel`
  (`vxn2-ui-web/assets/index.html:350-412`) with one tabbed
  `.fx-panel`. Tabs: Phaser / Delay / Reverb (signal order). Per-tab
  inline on/off switch follows the active tab's enable param. Port
  `wireFxTabs` + `fx-tabs.test.js` from vxn-1's `vxn-ui-web`.
- Preset round-trip: vxn-2 presets are name-keyed sparse TOML
  (per [[vxn2-preset-system]]) — new `phaser-*` keys default-fill on
  load, old presets load unchanged with phaser off. No factory
  migration needed.

## Out of scope

- **Phaser as a mod-matrix destination.** Not added to `DestId`
  (`vxn2-engine/src/matrix.rs:345`) or `DEST_NAMES`. Host-automation
  only, by request. If LFO-swept depth is wanted later, open a
  follow-up.
- Chorus. Stacking already provides ensemble thickening; a bolt-on
  chorus would double up. Phaser covers the swept-comb texture
  stacking can't. (Decision recorded here so it isn't re-litigated.)
- Phaser stages / centre / spread / width / jitter as user knobs
  (held at internal defaults, as in vxn-1).
- Per-FX send levels, parallel busses, reorderable chain.
- Touching reverb or adding a drift knob (vxn-1 E009 only).
- Cross-fade on tab switch — tabs toggle visibility; an inactive
  tab's DSP still runs if its `on` param is `1` (hiding ≠ bypassing).

## Phasing

- **0087** DSP — port `StereoPhaser` into `vxn2-dsp::phaser` with
  the collapsed param surface. Upstream tests ported. Self-contained.
- **0088** Params — append five `phaser-*` ids to the CLAP table,
  add `OFF_PHASER`, `PhaserParams` struct + decode in `shared.rs`,
  preset round-trip (old presets default-fill phaser off). Confirm
  **no** mod-matrix dest added.
- **0089** Engine bus — insert phaser pre-delay, wire
  `apply_block_params()` → `set_params`, clear state in reset, gate
  on `phaser-on`. Smoothed rate glide.
- **0090** Faceplate — collapse delay+reverb panels into one tabbed
  FX panel + phaser pane; port `wireFxTabs` + `fx-tabs.test.js`; CSS
  for the tab strip.

## Dependency order

```text
0087 (phaser DSP) ── 0088 (params) ── 0089 (engine bus) ── 0090 (faceplate)
0087 first because 0088's param decode references the phaser param
shape; 0088 before 0089 because the bus reads decoded params; 0090
last because the faceplate drives the new param ids.
```

## Acceptance

- `cargo test --workspace` passes (ported phaser tests included).
- `cargo build -p vxn2-clap --release` produces a CLAP that loads
  with one tabbed FX panel, three tabs (Phaser / Delay / Reverb)
  reachable, all phaser knobs driving DSP and exposed as host
  automation.
- `phaser-on = 0` → bus output bit-identical to pre-epic
  (deterministic null test).
- Phaser params do **not** appear as mod-matrix destinations
  (`grep -i phaser vxn-2/crates/vxn2-engine/src/matrix.rs` returns
  nothing).
- Old presets load with phaser defaulted off; saved DAW sessions
  with existing delay/reverb automation still resolve (param ids
  unchanged — phaser appended, not inserted).

## Close-out (2026-06-22)

All four tickets closed (0087–0090). Shipped: `StereoPhaser` ported into
`vxn2-dsp` (deps adapted, byte-identical stage scatter), five host-automation
`phaser-*` params appended at the table tail (blob v14 migration, NOT a
mod-matrix dest), inserted into the engine bus pre-delay
(`cleanup → phaser → delay → reverb`) with a bit-exact `phaser-on = 0` null,
and the delay/reverb panels collapsed into one tabbed FX panel (Phaser /
Delay / Reverb) with `wireFxTabs` + a vitest port (6/6).

`cargo test --workspace` passes; `cargo build -p vxn2-clap --release` builds
clean. Manual Reaper audio check pending per [[verify-audio-in-reaper]].
