---
id: "0041"
title: HTML faceplate — Osc 1, Osc 2, Mixer panels (incl. waveform selectors)
priority: high
created: 2026-05-30
epic: E010
---

## Summary

Implement the Row 1 oscillator and mixer panels in the HTML faceplate:
Osc 1 (Wave, Oct, Semi, Fine, PW), Osc 2 (same set), Mixer (Osc1,
Osc2, Ring, Noise levels + noise colour). Introduce the JS primitive
controls reused across every later panel: vertical fader, rotary
waveform selector, segmented button group, switch, dropdown. Each
control posts `UiEvent` on edit, listens for `ParamChanged` view
events from the controller.

## Acceptance criteria

- [ ] JS control primitives implemented:
      - `Fader(id, label)` — vertical slider, pointer down/up
        brackets `BeginGesture` / `EndGesture`, drag posts
        `SetParamNorm`.
      - `WaveformKnob(id, label)` — rotary selector with the same
        glyph set as `wave_points` (Sine, Tri, Saw, Pulse, etc.).
      - `Switch(id, label)` — vertical toggle for bools.
      - `ButtonGroup(id, label, variants)` — for Oversample,
        CrossModType, AssignMode.
      - `Dropdown(id, label, variants)` — fallback for any other
        multi-variant enum.
- [ ] Osc 1 panel renders Wave (rotary), Oct (fader), Semi (fader),
      Fine (fader), PW (fader). Layer-aware: switches with the
      Upper/Lower edit-layer toggle (sourced from a `ViewEvent` not
      yet specced; for this ticket, hard-bind to Upper and leave the
      toggle wiring to a placeholder).
- [ ] Osc 2 panel — identical control set, different param IDs.
- [ ] Mixer panel — four faders (Osc1, Osc2, Ring, Noise) + Col
      switch (White/Pink, two-variant enum).
- [ ] Each control's value display reads from the descriptor
      `display` string carried in the `ParamChanged` ViewEvent.
- [ ] DAW automation moves the right controls (Rust → JS push).
- [ ] UI gestures bracket correctly (the host's automation lane
      shows a single recording, not per-pixel events).

## Notes

The "layer-aware" question — how does the HTML editor know it's
showing Upper vs Lower? — needs a new ViewEvent: `EditLayerChanged`.
File a follow-up if not already covered by `KeyModeChanged` in 0035.
For this ticket, hard-bind controls to Upper and leave a TODO; 0045
ties up the layer toggle along with the Voice panel (which also
shows per-layer).

The rotary WaveformKnob is the visually distinctive piece — its
arc-arranged glyphs are what makes the faceplate look like a faceplate.
Port the geometry from `wave_points` in vxn-ui/src/lib.rs verbatim
(it's already coordinates-only).
