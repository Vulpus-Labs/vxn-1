---
id: "0040"
title: HTML faceplate shell — 4-row grid, panel containers, faceplate gutters
status: open
priority: high
created: 2026-05-30
epic: E010
---

## Summary

Build the static HTML/CSS layout that matches the Vizia faceplate's
top-level geometry: banner, preset-bar slot (left empty for now,
filled by Vizia overlay during transition), four rows of panels each
156px tall, panels gap-6, row gap-8. Position-accurate against
`target/vxn-layout.jsonl` (dumped by the `layout-probe` feature).
Panels render as empty bordered boxes with the orange header bar;
controls land in 0041+.

## Acceptance criteria

- [ ] CSS grid (or flex) reproduces the row/panel geometry from
      `vxn-layout.jsonl`. Each row is 1004 wide × 156 tall, gap 8,
      banner 26 tall + 8 gap, preset bar 30 tall + 8 gap.
- [ ] Per-row panel widths match the Vizia stretch shares
      documented in `panel_view`'s `match title` block (Bend = 54px
      fixed, Osc 1/Osc 2/LFO 1 each Stretch(1.2), etc.).
- [ ] Banner reads "VULPUS LABS — VXN-1", styled (1c1c1c bg, a7cfe2
      foreground, font-size 16, letter-spacing 3px) — matches the
      Vizia `.banner` class.
- [ ] Each panel container has the dark `1c1c1c` bg + `0e0e0e`
      border + 4px corner radius, with an orange `a7cfe2` header bar
      carrying the panel title (uppercased).
- [ ] Layer-dependent panels (Osc 1, Osc 2, Mixer, Filter, Env 1/2,
      VCA, Pitch Mod, PWM Mod, Cross Mod, Mod Wheel, Bend, Voice,
      LFO 1) carry a data attribute marking them so 0041+ can wire
      per-layer routing.
- [ ] Visual diff against a Vizia screenshot: panel positions
      within ±2 px in logical coordinates.
- [ ] Loads in `./deploy.sh --webview` (Phase B), opens in a DAW,
      shows the row grid + headers, no JS errors in DevTools.

## Notes

CSS variables for the per-panel widths and the row heights, named
after `FADER_H`/`COL_H`/`PANEL_H`/`DIAL` so the source mapping back
to Vizia constants is obvious. Don't bake pixel numbers into selectors
— a future resize policy should be one variable change.

Don't try to be pixel-clever about the header bar's toggle slot
(Chorus/Delay only). Reserve the layout space; the toggle widget
arrives with the FX panel in 0045.

The preset-bar slot stays empty in HTML; the Vizia editor's preset
bar overlays it during the E010→E011 transition. Add a transparent
placeholder div with the right height so the row grid stays aligned.
