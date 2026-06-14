---
id: E009
product: vxn-1
title: FX tab panel + phaser + FDN reverb + master drift
status: open
created: 2026-06-06
---

## Goal

Four coordinated changes to the FX/master surface:

1. **Consolidate Chorus / Delay / Reverb / Phaser into one
   `FX` panel** with a vertical tab selector on the left. Row 4
   shrinks from four FX panels + master to one FX panel + master.
2. **Add a Phaser** as the fourth FX tab. Port `VStereoPhaser`
   from `patches-bundles/patches-vintage`. Macro surface: Rate,
   Depth, FB, Mix (+ header on/off).
3. **Swap the BBD reverb for the vxn2 FDN reverb.** Drop the
   current `ReverbType` macro; expose `size`, `decay`, `damp`,
   `mix` as four knobs. Delete `crates/vxn-dsp/src/bbd.rs`
   entirely (no other consumer).
4. **Expose drift as a master knob.** `Engine::drift_amount`
   was internal with `DEFAULT_DRIFT_AMOUNT`; promote to a
   `GlobalParam::MasterDrift` (0..1, default 0.0). Knob lives
   in the Master panel alongside Tune/Volume.

## Background

The current row-4 panel-per-effect layout was tractable at three
effects; at four it would crowd the master out. Tabs reclaim the
horizontal budget, and keep room for a Drift knob in Master.

The BBD reverb (E012) shipped but reads as the wrong flavour for
this synth — comb-resonant and metallic when most factory
patches want a clean tail. The vxn2 FDN
(`vxn-2/crates/vxn2-dsp/src/reverb.rs`) is the clean Jot-style
8-line / Hadamard / per-line LP-damped network that the patches
were aiming for; lifting it into `vxn-dsp` is a straight port —
the vxn1 `Smoothed` smoother substitutes for vxn2's.

Phaser is missing from `patches-dsp`; the production phaser lives
at `patches-bundles/patches-vintage/src/vstereo_phaser.rs` —
two-cascade allpass chain sharing one LFO, stereo spread via L/R
phase offset, signed feedback, optional jitter. The upstream
surface (rate, depth, center, feedback, mix, spread, width,
jitter, stages) is too wide; collapse to Rate / Depth / FB / Mix
with sensible internal defaults (centre fixed mid-band, stages
fixed at 4, spread fixed at antiphase, width neutral, jitter 0).

Drift exists internally as `Engine::drift_amount` at
`DEFAULT_DRIFT_AMOUNT`. Promoting it to a `GlobalParam` is one
enum slot, one `set_param` arm, and one knob — small change, big
expressive payoff (per-voice pitch jitter is the cheap "analog"
knob).

## In scope

- New `vxn-dsp::phaser::StereoPhaser` ported from
  `patches-bundles/patches-vintage::VStereoPhaser`. Upstream
  tests ported. Host-rate only.
- New `vxn-dsp::fdn_reverb::FdnReverb` ported from
  `vxn2-dsp::reverb::Reverb`. `Smoothed` swapped for vxn-dsp's.
- Deletion of `vxn-dsp/src/bbd.rs` and its `mod bbd` line.
  `random_walk.rs` already lives outside `bbd.rs` (per git
  status) — no port needed.
- Param table churn:
  - **Drop** `ReverbType`, `ReverbDepth`.
  - **Add** `ReverbSize`, `ReverbDecay`, `ReverbDamp`,
    `MasterDrift`, `PhaserOn`, `PhaserRate`, `PhaserDepth`,
    `PhaserFB`, `PhaserMix`.
  - `ReverbOn`, `ReverbMix` keep their names.
  - Per [[vxn1-id-stability-dropped]], no append-only
    discipline — re-order freely to keep the table readable.
- Engine bus: insert phaser **pre-chorus** so the canonical chain
  is `dry → phaser → chorus → delay → reverb → limiter`. Wire
  `MasterDrift` directly into `Engine::drift_amount`.
- Faceplate row 4: replace four FX panels with one tabbed FX
  panel (vertical tab strip on the left, single content area on
  the right). Tabs: Phaser / Chorus / Delay / Reverb. Header
  on/off switch follows the active tab's enable param. Master
  panel gains a Drift knob.
- Preset format: drop `reverb_type` / `reverb_depth` keys (name-
  keyed presets — old keys become unknown and are ignored, new
  keys default-fill on load per [[vxn1-preset-system]]). Add
  factory bank audit — no need to migrate values, just re-save
  the bank with the new defaults so reverb tails sound right.

## Out of scope

- Sub-bus routing (still chorus → delay → reverb fixed; no per-FX
  send levels, no parallel busses).
- Phaser stages / spread / width / jitter as user knobs (held at
  internal defaults). If the v1 phaser feels generic we open a
  follow-up.
- Reverb pre-delay knob (out — FDN base lengths cover it).
- Cross-fade on tab switch (tabs just toggle visibility; the DSP
  for an inactive tab still runs if its `on` param is `1` —
  hiding ≠ bypassing).
- Touching `vxn-1/crates/vxn-dsp/src/bbd.rs` history beyond
  deletion — no ADR for the BBD removal, the E012 epic note is
  enough trail.
- ADR for the FDN port — `vxn-2`'s ADR (referenced in the FDN
  source header) is the source of truth; `vxn-1` cribs it.
- ADR for the tab idiom — if it generalises (mod matrix, perf
  view) we open one then.

## Phasing

- **0094** DSP — port `VStereoPhaser` core into
  `vxn-dsp::phaser` with collapsed param surface. Upstream tests
  ported. Self-contained.
- **0095** DSP — port `vxn2-dsp::reverb` as
  `vxn-dsp::fdn_reverb`. Delete `bbd.rs` + `mod bbd`. Update
  `vxn-dsp/src/lib.rs` exports.
- **0096** Params — rewrite `GlobalParam` enum (drop two,
  add nine). Update `GLOBAL_PARAMS` desc table. Update CLAP id
  layout consumers. Update preset round-trip — old keys ignored,
  new keys default-fill.
- **0097** Engine bus — swap reverb field type, insert phaser
  pre-chorus, wire `MasterDrift` into `Engine::drift_amount`,
  wire smoothed `size/decay/damp/mix` for reverb and
  `rate/depth/fb/mix` for phaser. Update `update_effects`.
- **0098** Faceplate — replace four row-4 FX panels with one
  tabbed FX panel + Drift knob in Master. Update CSS for vertical
  tab strip. JS dispatch routes tab clicks to swap visible body.
- **0099** Factory bank — re-save factory presets so the FDN
  reverb defaults sound right; close E012 ticket 0059 (BBD
  factory tasting) as superseded.

## Dependency order

```text
0094 (phaser DSP)  ─┐
0095 (FDN reverb)  ─┤  all DSP can land in parallel
                    ├── 0096 (params: drop+add) ── 0097 (engine bus) ── 0098 (faceplate) ── 0099 (factory)
0094 + 0095 first because 0096's GlobalParam churn references the
new DSP types' param shapes.
```

## Acceptance

- `cargo test --workspace` passes.
- `cargo build -p vxn-clap --release` produces a CLAP that loads
  with the new FX panel visible, four tabs reachable, all knobs
  driving DSP.
- Old presets load with reverb defaulted to off (or the new
  default voicing if `reverb_on` was `1`) and don't reference
  removed keys.
- Drift knob at 0 → bit-identical to current `drift_amount = 0`
  (deterministic); at default `DEFAULT_DRIFT_AMOUNT` → matches
  current "live" detune.
- `bbd.rs` is gone; `grep -r 'StereoVReverb\|use crate::bbd' vxn-1/crates`
  returns nothing.
