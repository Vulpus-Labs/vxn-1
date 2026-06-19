---
id: "0001"
product: vxn-1
title: Fixed-panel editor rebuild
priority: medium
created: 2026-05-25
epic: E001
---

## Summary

Rebuild the [`vxn-ui`] editor around **fixed, labelled panels** (JP-8/Juno
idiom), replacing the generic matrix controls. Surfaces the param model from
0022 and the new ring level (0021) / cross-mod type selector.

Depends on **0022** (param model) and **0021** (RingLevel). Land last in E001.

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

- [x] All panels above render and bind to the 0022 params; every automatable
      param has exactly one control; no orphaned/dead controls. *(Enforced by
      `every_automatable_param_has_exactly_one_control` — expands `ROWS` and
      asserts each of `TOTAL_PARAMS` ids is bound exactly once. The E003 voice/
      glide params (`AssignMode`/`UnisonDetune`/`Portamento*`), live + automatable
      but absent from ADR 0004's panel list, got a dedicated **Voice** panel so
      no automatable param is orphaned.)*
- [x] Cross-mod type selector switches Off/Sync/PM; amount knob drives the PM
      index. *(Segmented button group; descriptor already labels the modes
      `Off/Sync/FM`. Amount knob dims + goes non-interactive when type = Off
      via `cross_mod_dim`.)*
- [x] Mixer shows osc1/osc2/ring/noise + two-button noise selector. *(Osc levels
      moved out of the osc panels into Mixer; `NoiseColor` now renders as a
      White/Pink button group, not a switch.)*
- [x] Filter panel includes drive and key-track toggle; filter-mod panel shows
      velocity/LFO/env into cutoff; mod-wheel panel shows the mod-wheel depths.
      *(Old combined Filter panel split into **Filter** + **Filter Mod**. Mod
      Wheel surfaces all four depths — PWM/cutoff/reso/Osc2 — per ADR 0004 §4.)*
- [x] Per-layer switching + host gesture recording still work. *(The
      `resolve`/`is_layer_dependent`/`on_idle` machinery and the fader/knob
      gesture plumbing are untouched; existing tests still pass.)*
- [x] No RT work on the audio thread from the editor (UI thread only). *(Editor
      only writes the shared store + raises gestures, as before.)*

### Outstanding / deferred

- **Manual host validation** (load `.clap`, exercise each control, confirm
  automation round-trips) is the one step that can't run headless — needs a DAW.
- **Source selectors kept as dropdowns**, not the segmented selectors the Design
  notes suggest: the Osc Mod panel already carries 5 selectors + 6 faders, and
  segmented 3-way groups would overflow the row. Functionally complete (bound +
  automation-synced); the segmented styling is a cosmetic follow-up.

## Notes

- Validation: build the plugin, load in a host, exercise each control; confirm
  automation round-trips (`value_to_text` / gesture). `cargo test -p vxn-ui`.

## Close-out (2026-06-19)

- All fixed panels ship in the webview faceplate (Osc 1/2, Osc Mod, Mixer,
  Filter, Filter Mod, Mod Wheel + a Voice panel for E003 params). Param model
  from 0022 surfaced: `CrossModType {Off,Sync,Pm,Ring}` at
  [params.rs:101](../../vxn-1/crates/vxn-app/src/params.rs#L101); 24-cell matrix
  gone (no `MATRIX_BASE`/`ModSource`/`ModDest`), routing via `resolve_mod`
  ([voice.rs:1366](../../vxn-1/crates/vxn-engine/src/voice.rs#L1366)).
- Every automatable param bound exactly once — enforced by
  `every_automatable_param_has_exactly_one_control`.
- Mixer ring/noise + White/Pink button group; brown noise dropped (0021).
- Manual DAW validation done (Reaper): controls exercised, automation
  round-trips, layout settled.
- Deferred (cosmetic, non-blocking): Osc Mod source selectors render as
  dropdowns rather than segmented 3-way groups — functionally complete and
  automation-synced.
