---
id: E028
product: vxn-2
title: FX dynamics block (compressor + saturation)
status: closed
created: 2026-06-24
---

## Goal

Add a fourth FX, **Dynamics**, providing a feed-forward compressor
(threshold / ratio / attack / release / makeup) and a saturator
(drive). Inserted **first** in the FX bus, with the same on/off
fade-in / fade-out crossfade semantics as the existing phaser / delay
/ reverb (`set_enabled` retargets an internal wet-mix smoother; the
block only reverts to a bit-exact passthrough once the fade-out hits
zero). Surfaced as a fourth tab in the existing tabbed FX panel
(E025).

Dynamics params are **host-automation only** — appended to the flat
CLAP param table but not added as mod-matrix destinations (same
discipline as the phaser; see E025 out-of-scope).

## Background

Current bus (`vxn-2/crates/vxn2-engine/src/engine.rs:1449-1451`):

```
cleanup → phaser → delay → reverb → master gain → limiter
```

There is no dynamics-shaping stage on the FX bus. Patches with
aggressive FM transients hit the time FX raw — the delay regen and
reverb tail accumulate uneven peaks; the master limiter
(`vxn2_dsp::limiter::StereoLimiter`, gated by `master.limiter_on`)
is a *brickwall safety*, not a musical comp, and runs **after**
master gain so it can't shape what the FX bus feeds.

A musical comp + saturation block placed **first** in the bus
(channel-strip topology) gives:

- comp evens FM transients before delay regen / reverb tail
  accumulate them;
- saturation drives the phaser allpasses with a coloured signal
  (classic "overdrive into modulation" feel) rather than smearing
  already-modulated harmonics;
- the brickwall limiter remains where it is, doing its different
  job.

The fade semantics are already a solved pattern in vxn-2: phaser
(`vxn-2/crates/vxn2-dsp/src/phaser.rs:347-394`), delay
(`vxn-2/crates/vxn2-dsp/src/delay.rs:265-291`), reverb
(`vxn-2/crates/vxn2-dsp/src/reverb.rs:289-340`) all retarget a
`Smoothed` mix to 0 on `set_enabled(false)`, early-return bit-exact
passthrough only when `mix.current() == 0.0`, and snap (no startup
fade) on the first `set_from`. Dynamics follows the same shape with
a single block-level wet/dry smoother wrapping comp+sat.

## In scope

- New `vxn2-dsp::dynamics::DynamicsBlock` (stereo, internal order
  **comp → sat**). Feed-forward peak detector, soft-knee VCA-style
  comp, makeup gain, then a `tanh`-flavoured saturator with input
  drive in dB. Smoothed `wet` mix (0..1) glides on enable/disable
  exactly like phaser/delay/reverb. Reset on enable from a
  fully-faded-out state (no envelope-follower residue).
- Param table growth: **append** eight `dyn-*` ids (`dyn-on`,
  `dyn-threshold`, `dyn-ratio`, `dyn-attack`, `dyn-release`,
  `dyn-makeup`, `dyn-drive`, `dyn-mix`) at the **end** of the flat
  CLAP table (new blob `v15`). Appending — not inserting — keeps
  every existing id (filter, phaser, limiter) stable so saved DAW
  automation survives. New `OFF_DYNAMICS` section offset,
  `DynamicsParams` struct, decode arm in `shared.rs`.
- Engine bus: insert dynamics **first** so the chain becomes
  `cleanup → dynamics → phaser → delay → reverb → master → limiter`.
  Wire through `apply_block_params()` → `dynamics.set_params(...)`;
  clear envelope-follower state in `Synth::reset`. `dyn-on = 0`
  must keep the bus sample-exact (gated like `phaser-on`).
- Faceplate: add a fourth tab `Dyn` to the existing `.fx-panel`
  (left of Phaser — signal order). Per-tab inline on/off switch
  bound to `dyn-on`. Knobs/faders for the six continuous params +
  mix. CSS reuses the existing tab-strip; no new idiom.
- Preset round-trip: vxn-2 presets are name-keyed sparse TOML
  (per [[vxn2-preset-system]]) — new `dyn-*` keys default-fill on
  load, old presets load unchanged with dynamics off. No factory
  migration needed.
- PARAMETERS.md: add a `### Dynamics` subsection under `## Effects`
  alongside Delay / Reverb (and Phaser, if added in this epic —
  noting [[vxn2-parameters-md-phaser-gap]] if the doc still hasn't
  been backfilled from E025).

## Out of scope

- **Dynamics as a mod-matrix destination.** Not added to `DestId`
  (`vxn-2/crates/vxn2-engine/src/matrix.rs`). Host-automation only,
  matching the phaser precedent (E025). If LFO-swept drive is
  wanted later, open a follow-up.
- Lookahead / oversampling on the comp or saturator. Feed-forward
  peak detector at base rate; tanh saturator at base rate. Aliasing
  from extreme drive accepted as a v1 tradeoff.
- Knee shape, detector RMS-vs-peak, sat type (tanh vs. tape vs.
  diode) as user knobs — held at internal defaults (mirrors phaser
  pinning stages/centre/spread internally).
- Sidechain input or sidechain HPF.
- Reordering the FX chain (dynamics is hard-wired first).
- Replacing or removing the master brickwall limiter (different
  job; stays where it is).

## Phasing

- **0145** DSP — `vxn2-dsp::dynamics::DynamicsBlock` with the eight
  collapsed params, wet-mix smoother, `set_enabled` fade semantics
  matching phaser/delay/reverb. Unit tests: fade-out reaches
  bit-exact passthrough; fade-in starts from 0 wet; gain reduction
  matches a known threshold/ratio target; tanh drive flattens a
  known sine peak; switch-off then switch-on resets the envelope
  follower so there's no residual gain reduction.
- **0146** Params — append eight `dyn-*` ids to the CLAP table, bump
  blob to v15, add `OFF_DYNAMICS`, `DynamicsParams` struct + decode
  in `shared.rs`, preset round-trip (old presets default-fill
  dynamics off). Confirm **no** mod-matrix dest added. Update
  PARAMETERS.md.
- **0147** Engine bus — insert dynamics first in the chain
  (`cleanup → dynamics → phaser → delay → reverb`), wire
  `apply_block_params()` → `set_params`, clear in reset, gate on
  `dyn-on`. Bit-exact null vs. pre-epic when `dyn-on = 0`.
- **0148** Faceplate — add a fourth `Dyn` tab to the FX panel, left
  of Phaser. Per-tab on/off switch bound to `dyn-on`. Knobs for
  threshold / ratio / attack / release / makeup / drive / mix.

## Dependency order

```text
0145 (dynamics DSP) ── 0146 (params) ── 0147 (engine bus) ── 0148 (faceplate)
```

0145 first because 0146's decode references the dynamics param
shape; 0146 before 0147 because the bus reads decoded params; 0148
last because the faceplate drives the new param ids.

## Acceptance

- `cargo test --workspace` passes (new dynamics DSP tests included).
- `cargo build -p vxn2-clap --release` produces a CLAP that loads
  with four tabs (Dyn / Phaser / Delay / Reverb) reachable, all
  dynamics knobs driving DSP and exposed as host automation.
- `dyn-on = 0` → bus output bit-identical to pre-epic
  (deterministic null test).
- Dynamics params do **not** appear as mod-matrix destinations
  (`grep -i 'dyn\|dynamics' vxn-2/crates/vxn2-engine/src/matrix.rs`
  returns nothing).
- Old presets load with dynamics defaulted off; saved DAW sessions
  with existing phaser/delay/reverb/filter/limiter automation still
  resolve (param ids unchanged — dynamics appended, not inserted).
- Manual Reaper audio check per [[verify-audio-in-reaper]] confirms
  no clicks on `dyn-on` toggle (fade semantics).
