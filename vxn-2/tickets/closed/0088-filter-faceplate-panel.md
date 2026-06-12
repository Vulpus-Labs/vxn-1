---
id: "0088"
title: "Filter faceplate panel — controls + FX-style enable toggle"
priority: medium
created: 2026-06-12
epic: E007
depends: ["0083"]
---

## Summary

Eighth ticket of [E007](../../epics/open/E007-optional-per-voice-filter.md).
[0083](0083-filter-params-and-matrix-dests.md) adds the filter params and matrix
dests, but the feature otherwise ships **headless** — no control on the
faceplate. E003 closed on the invariant "the faceplate covers every param in
`PARAMETERS.md`"; the filter must not break it. Add a filter section to the HTML
faceplate (`vxn2-ui-web`) so every filter param is reachable and the optional
filter reads visually like the optional FX.

The matrix-routing half is already free: `Cutoff` / `Resonance` auto-surface in
the mod-matrix overlay because its dest dropdown is data-driven
(`window.__vxn.matrix.dests`, [mod-matrix.js](../../crates/vxn2-ui-web/assets/panels/mod-matrix.js#L89)).
This ticket is only the filter *module* panel.

## Design

New panel `assets/panels/filter.js` + its section in the faceplate HTML,
reusing existing widgets (no new param-wiring path — `data-vxn-param`
machine-ids resolve through the standard `resolveParamId`):

- `fader.js` — `filter-cutoff` (log), `filter-resonance`, `filter-drive`.
- `button-group.js` — `filter-mode` (LP/HP/BP/Notch), `filter-slope` (2/4-pole),
  `filter-oversample` (1×/2×/4×/8×).
- Header switch — `filter-enable`, using the FX header-toggle idiom (header
  colour change + body dim when off) so "optional like the FX" is visually
  literal.

**Layout home:** the faceplate is FM-shaped (op row / mod row / perf row); the
filter is a new module. Place it in the **performance row beside the FX**, so
the synth → filter → FX signal order reads left-to-right and the two optional
header-toggle modules sit together. (See Notes — pin placement before building
markup if it crowds the perf row.)

Host automation echo + right-click numeric entry come for free via the existing
controller/IPC path; verify cutoff/reso/drive round-trip and that the structural
selectors (mode/slope/OS/enable) commit as patch state.

## Acceptance criteria

- [x] Every filter param has a faceplate control; `PARAMETERS.md` "every param
  reachable" invariant restored (coverage test extended with all 7 `filter-*`
  ids + the `filter` section).
- [x] `filter-enable` toggle dims the panel body and recolours its header when
  off — reuses the FX `.panel-header.toggleable` + `.panel.toggle-off` idiom
  (default off ⇒ panel renders dimmed). No JS: the generic `bindToggleHeaders`
  binds it like `delay-on`.
- [x] `cutoff` / `resonance` / `drive` echo host automation + right-click numeric
  entry via the standard fader binder (`.fader[data-vxn-param]`) — same path as
  every other continuous control, no new wiring.
- [x] `mode` / `slope` / `oversample` selectors are `.bgrp-row[data-vxn-param]`
  button groups bound by `bindButtonGroups`; they're CLAP/patch params (0083) so
  they persist + reload through the preset/blob path like `lfo2-shape`.
- [x] `Cutoff` / `Resonance` appear as routable mod-matrix dests (data-driven
  dropdown; ui-web test asserts the dest list now has 30 entries incl `cutoff` /
  `resonance`) — no JS change, verification only.
- [x] Panel renders against the Init patch with the filter off (default); pure
  declarative HTML reusing existing widgets/CSS. **Visual A/B in a DAW is the
  user's check** — markup is structurally consistent (faders + cgrp selectors
  mirror the stack/voice panels); perf-row flex rebalanced to fit 6 panels.

## Notes

If the perf row is too crowded to take the filter cleanly, the fallback is its
own short strip between the mod row and perf row; decide before writing markup.
This ticket has no DSP dependency beyond params — it can build in parallel with
the render path ([0084](0084-per-stack-filter-render-path.md)); the control just
drives an inert param until 0084 lands.
