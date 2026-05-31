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

## Status

**Partially complete.** This commit lands the infrastructure (timer
pump, descriptor push, dispatch) plus Osc 1. Osc 2, Mixer, and the
remaining three primitives roll into a follow-up (0041a).

## Acceptance criteria

- [ ] JS control primitives implemented:
      - [x] `Fader(id, label)` — vertical slider, pointer down/up
            brackets `BeginGesture` / `EndGesture`, drag posts
            `SetParamNorm`. Uses pointer-capture so the drag tracks
            past the track edges.
      - [x] `WaveformKnob(id, label)` — rotary selector with the same
            glyph set as `wave_points` (Sine, Tri, Saw, Pulse, etc.).
            Glyphs arranged on a -120°…+120° arc; click selects.
      - [ ] `Switch(id, label)` — vertical toggle for bools.
      - [ ] `ButtonGroup(id, label, variants)` — for Oversample,
            CrossModType, AssignMode.
      - [ ] `Dropdown(id, label, variants)` — fallback for any other
            multi-variant enum.
- [x] Osc 1 panel renders Wave (rotary), Oct (fader), Semi (fader),
      Fine (fader), PW (fader). Layer hard-bound to Upper; the
      Upper/Lower edit-layer toggle is deferred to 0045 along with
      the Voice panel.
- [ ] Osc 2 panel — identical control set, different param IDs.
      Deferred to 0041a; the primitives + plumbing are reusable.
- [ ] Mixer panel — four faders (Osc1, Osc2, Ring, Noise) + Col
      switch (White/Pink, two-variant enum). Deferred to 0041a
      (needs the Switch primitive).
- [x] Each control's value display reads from the descriptor
      `display` string carried in the `ParamChanged` ViewEvent (the
      Vizia formatting routes through the same controller path).
- [x] DAW automation moves the right controls (Rust → JS push) via
      the CLAP `timer-support` extension. The clack shell registers
      a ~16 ms (60 Hz) main-thread timer in `set_parent`; `on_timer`
      ticks the controller and drains `view_rx` into
      `EditorHandle::push_view_event`.
- [x] UI gestures bracket correctly — Fader pointerdown posts
      `begin_gesture`, pointerup/cancel posts `end_gesture`; the
      WaveformKnob's discrete write is wrapped in a `begin/end`
      pair too so the host records one edit, not zero.

## Follow-up: 0041a

- Switch / ButtonGroup / Dropdown primitives (CSS + JS pattern is
  set by Fader; mostly mechanical).
- Osc 2 panel (clone of Osc 1 with `osc2_*` param names).
- Mixer panel (Osc1Level/Osc2Level/RingLevel/NoiseLevel faders +
  NoiseColor switch).

## Architecture notes (kept here for the follow-ups)

The bridge is established. Subsequent panel tickets only need to:

1. Drop control mount points into the panel body with
   `data-control="..." data-param="<descriptor.name>"
   data-label="..."`.
2. If the control type is new, add a `makeFoo(el, id, desc)` factory
   to `assets/faceplate.html` that builds DOM + binds events and
   returns `{ update(plain, norm, display) }`.
3. Add the kind to the `init()` switch.

No new Rust code required per panel; the descriptor JSON push +
ViewEvent dispatch cover routing automatically.

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
