---
id: "0044"
title: HTML faceplate — Pitch Mod, PWM Mod, Cross Mod, Mod Wheel, Bend panels
priority: high
created: 2026-05-30
epic: E010
---

## Summary

Implement Row 3: the modulation routing panels. Pitch Mod and PWM
Mod each carry an LFO source selector + depth and an Env source
selector + depth (laid out as route columns — selector beneath fader).
Cross Mod is the wide custom panel: Type (Off/Sync/PM) + Amount,
beside the osc2-only pitch route (source selector + depth). Mod
Wheel routes to PWM, Cutoff, Reso, Osc 2 Pitch. Bend is a single-fader
panel for pitch wheel range.

## Acceptance criteria

- [ ] Pitch Mod panel: two route columns (LFO depth + source, Env
      depth + source). Selector dim-grey when source is `Off`.
- [ ] PWM Mod panel: same structure, different params.
- [ ] Cross Mod panel: Type selector (segmented buttons Off/Sync/PM)
      + Amount fader, beside Src (source dropdown) + Mod (depth
      fader). Amount fader dims when Type=Off. Layer-aware.
- [ ] Mod Wheel panel: four faders (PWM, Cutoff, Reso, O2 Pitch).
- [ ] Bend panel: single Range fader, 54px wide (panel pinned narrow).
- [ ] Source selectors post `SetParam` with the variant index;
      depth faders post `SetParamNorm` like any other fader.
- [ ] Dim-when-Off visual driven by `ParamChanged` event on the
      source selector — no client state.

## Notes

The route column ("fader on top, source selector beneath") is a
recurring layout idiom worth a reusable JS primitive
`RouteColumn(headerLabel, sourceParam, depthParam)`.

Cross Mod's custom layout (no plain row of cells) was
`cross_mod_panel` in Vizia — port the structure but as
HTML/CSS, not as a procedural builder.
