---
id: "0045"
title: HTML faceplate — Voice, Master, Chorus, Delay panels + layer toggle wiring
priority: high
created: 2026-05-30
epic: E010
---

## Summary

Implement Row 4's remaining panels (Keys slot stays Vizia for now,
E011): Voice (assign mode / unison detune / legato / glide), Master
(tune / volume / limit / oversample), Chorus (with header on/off
toggle), Delay (with header on/off toggle). Also wire the
`EditLayerChanged` ViewEvent so layer-aware controls actually flip
between Upper and Lower edit targets across all 0041–0044 panels.

## Acceptance criteria

- [ ] Voice panel: AssignMode (segmented Poly/Twin/Unison/Solo), the
      Legato toggle drawn inside the Detune cell (4th row, below the
      fader), Detune fader (mode-aware top: 50ct in Unison, 20ct in
      Twin), Glide fader.
- [ ] Master panel: Tune, Volume (faders); Limit (switch),
      Oversample (segmented buttons 1×/2×/4×/8×).
- [ ] Chorus panel: header switch (`On`), Rate, Depth, Mix faders.
- [ ] Delay panel: header switch (`On`), Time, Sync, FB, Mix, PingPong
      controls.
- [ ] `EditLayerChanged { layer }` ViewEvent added to vxn-app +
      emitted by controller when the layer toggle changes;
      consumed by the editor to rebind layer-aware panels to the
      new layer's CLAP IDs.
- [ ] Detune fader's top changes when the assign-mode switches into
      Twin (per Vizia's `detune_top`); the engine's clamp behaviour
      stays in the controller — the view only renormalises its
      display.

## Notes

`EditLayerChanged` is a new ViewEvent — add it to vxn-app's enum
and the controller's emit list (it's purely view state, doesn't
mutate any param; just a snapshot of which layer the UI is editing).

Detune top: the descriptor's max is 50ct (Unison). In Twin the
useful range is 20ct. The Vizia editor solved this by mapping the
fader's [0,1] to [0,20ct] dynamically; the WebView editor does the
same — `Fader` accepts a `topOverride` set per render-cycle based on
the assign mode it last saw.

The header-switch idiom (Chorus/Delay) is the only place the panel
header carries an interactive control. The faceplate shell needs to
host this: the panel container's `<header>` gets a toggle row plus
the title.
