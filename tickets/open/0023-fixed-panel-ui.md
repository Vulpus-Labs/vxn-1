---
id: "0023"
title: Fixed-panel editor rebuild
priority: medium
created: 2026-05-25
epic: E006
---

## Summary

Rebuild the [`vxn-ui`] editor around **fixed, labelled panels** (JP-8/Juno
idiom), replacing the generic matrix controls. Surfaces the param model from
0022 and the new ring level (0021) / cross-mod type selector.

Depends on **0022** (param model) and **0021** (RingLevel). Land last in E006.

## Panels

- **Osc 1:** wave selector, octave / coarse / fine, pulse width.
- **Osc 2:** wave selector, octave / coarse / fine, pulse width, **cross-mod
  type {Off / Sync / PM}** selector + **amount** knob. (PM = through-zero phase
  mod, ADR 0004 §7; the on-panel label may read "FM" since players expect it.)
- **Osc mod:**
  - Pitch (both oscs, vibrato scale) ← LFO source {Off/LFO1/LFO2} + depth, Env
    source {Off/Env1/Env2} + depth, pitch-wheel depth.
  - PWM ← LFO source + depth, Env source + depth.
  - Osc 2 pitch (wide / sync sweep, octave range) ← Env source {Off/Env1/Env2}
    with its own depth.
- **Mixer:** osc1, osc2, **ring**, noise level faders + **noise type** as two
  small buttons (White / Pink).
- **Filter:** HP cutoff, LP cutoff, resonance, **drive**, **key-track** on/off.
- **Filter mod:** cutoff ← velocity depth, LFO source + depth, env source +
  depth.
- **Mod wheel:** mod→PWM, mod→cutoff, mod→reso, mod→Osc2 pitch (octave range,
  sync sweeps) depths.

Keep the existing per-layer (Upper/Lower) display switching, the host-aware
gesture plumbing, and the LFO 1 / LFO 2 panels from E004/E005 (per-voice LFO1
delay/fade/free-run; global LFO2). The source selectors here let either LFO feed
the channels.

## Design notes

- The {Off/LFO1/LFO2} and {Off/Env1/Env2} selectors map to the `*LfoSrc` /
  `*EnvSrc` enum params; render as small segmented selectors next to each depth
  control.
- Noise type = two momentary/toggle buttons bound to the White/Pink `NoiseColor`
  enum (no third state — brown dropped in 0021).
- Cross-mod type = three-way selector; the amount knob can grey out / disable
  when type = Off (cosmetic, optional).
- Reuse the existing fader/rotary/switch/enum bound-control + `on_idle` sync
  machinery; no new host-binding mechanism.

## Acceptance criteria

- [ ] All panels above render and bind to the 0022 params; every automatable
      param has exactly one control; no orphaned/dead controls.
- [ ] Cross-mod type selector switches Off/Sync/PM; amount knob drives the PM
      index.
- [ ] Mixer shows osc1/osc2/ring/noise + two-button noise selector.
- [ ] Filter panel includes drive and key-track toggle; filter-mod panel shows
      velocity/LFO/env into cutoff; mod-wheel panel shows the three mod-wheel
      depths.
- [ ] Per-layer switching + host gesture recording still work.
- [ ] No RT work on the audio thread from the editor (UI thread only).

## Notes

- Validation: build the plugin, load in a host, exercise each control; confirm
  automation round-trips (`value_to_text` / gesture). `cargo test -p vxn-ui`.
