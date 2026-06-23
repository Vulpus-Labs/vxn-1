---
id: "0141"
product: monorepo
title: Web faceplate — split op-row.js and panels.js god-files
priority: medium
created: 2026-06-23
epic: E027
---

## Summary

Two faceplate files own too much. Behaviour-preserving DOM
refactor.

1. **vxn-2 `panels/op-row.js` (842 lines)** fuses five
   concerns: ALGO_CARRIERS / ALGO_FB_OPS data tables
   (`:30-128`), the algo-grid overlay render + picker wiring
   (`:183-253`), the op-tab strip (`:255-278`), op-detail DOM
   construction (`:688-779`), and the KS-graph drag protocol
   (`:388-686`, 200+ lines — deep enough for its own module).
   Split: `panels/ks-graph.js`, `panels/eg-graph.js`,
   `panels/algo-data.js`; `op-row.js` becomes a ~250-line
   coordinator.
   - Also: `renderOpDetail` (`:688-779`) hardcodes column
     geometry in JS (`width:160px` `:700`, `width:188px`
     `:735`) via `style.cssText`/`innerHTML`. Move the
     op-detail skeleton to static HTML slots (or at least
     extract column geometry to CSS classes) so a layout
     change edits HTML/CSS, not JS.

2. **vxn-1 `panels.js` (1187 lines)** mixes panel IIFEs
   (`presetBar:27`, `keysPanel:188`), exported math/glyph
   constants (`:404`), generic drag/paint primitives, and
   discrete-input widgets (`makeSwitch:860`,
   `makeButtonGroup:911`, `makeDropdown:952`,
   `makeHeaderSwitch:986`). Re-shape toward vxn-2's modular
   `panels/` layout: `panels/fader.js`, `panels/keys.js`,
   `panels/preset-bar.js`, `util/drag.js` (the last consuming
   the shared `wireDrag` from `0140`).
   - Also drop the `presetBar` runtime monkey-patch of
     `window.vxn.send.*` (`panels.js:70-84`) — move
     dirty-tracking into `dispatch.js` as an `onMutation`
     hook instead of overwriting shared sender methods.

## Acceptance criteria

- [ ] `op-row.js` ≤ ~300 lines; KS-graph, EG-graph, and the
      ALGO data tables are separate modules; op-detail column
      geometry lives in CSS/HTML, not inline JS strings.
- [ ] `panels.js` is split into modular `panels/` files
      matching vxn-2's structure; the build step (splice/
      concat) is updated to load them.
- [ ] `presetBar` no longer reassigns `window.vxn.send.*`;
      dirty-tracking is an explicit hook in the dispatch
      layer.
- [ ] The fragile open/close race in vxn-2 (`main.js:500-525`
      `bindCustoms` vs `op-row.js:231` capture-phase
      `wireOverlayButtons`) is resolved — remove open/close
      handling from `CUSTOM_OPS`; the overlay owners wire
      their own.
- [ ] vitest green; new unit tests cover the KS-graph drag
      and the ALGO_CARRIERS/FB_OPS tables (currently
      untested).

## Notes

Depends on `0140` (consume the shared `wireDrag` / value-pop
rather than re-importing per-file copies). Behaviour must not
change — these are layout/structure moves; pin behaviour with
the new tests first where coverage is thin (op-row's
200-line KS drag is entirely untested today). vxn-2's modular
`panels/` is the target shape for both synths. The `op-row.js`
split touches the same file E026's ticket 0128 (EG-curve
selector) edits — sequence after that ticket lands, or
coordinate, to avoid a churn conflict.
