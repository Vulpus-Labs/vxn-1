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

## Close-out (2026-06-30)

Landed in two commits: vxn-2 half `19adc1c`, vxn-1 half `683d2d2`.
0128 + 0140 both already shipped, so no churn conflict.

- **vxn-2 op-row split.** `op-row.js` is now a 275-sloc coordinator
  (`grep -vcE '^\s*(//|$)'`); the heavy concerns live in sibling
  modules: [algo-data.js](../../vxn-2/crates/vxn2-ui-web/assets/panels/algo-data.js)
  (ALGO_CARRIERS / ALGO_FB_OPS / OP_PARAMS / isCarrier),
  [ks-graph.js](../../vxn-2/crates/vxn2-ui-web/assets/panels/ks-graph.js)
  (the 200-line KS drag protocol),
  [eg-graph.js](../../vxn-2/crates/vxn2-ui-web/assets/panels/eg-graph.js),
  and [op-faders.js](../../vxn-2/crates/vxn2-ui-web/assets/panels/op-faders.js)
  (per-op fader factory + Ratio/Fixed selector). Each takes a per-render
  binding context instead of closing over op-row's locals.
- **op-detail column geometry → CSS.** No `style.cssText` left in op-row
  (`grep -c` = 0); geometry is `.op-col-tuning` / `.op-col-graph` /
  `.op-col-senout` / `.op-col-sens` / `.op-col-out` / `.op-col-row-start`
  in [style.css](../../vxn-2/crates/vxn2-ui-web/assets/style.css).
- **vxn-1 panels.js split.** Split into
  [util/drag.js](../../vxn-1/crates/vxn-ui-web/assets/util/drag.js),
  [panels/fader.js](../../vxn-1/crates/vxn-ui-web/assets/panels/fader.js),
  [panels/discrete.js](../../vxn-1/crates/vxn-ui-web/assets/panels/discrete.js),
  [panels/keys.js](../../vxn-1/crates/vxn-ui-web/assets/panels/keys.js),
  [panels/preset-bar.js](../../vxn-1/crates/vxn-ui-web/assets/panels/preset-bar.js);
  `panels.js` is now a re-export barrel so the 11 vitest suites import
  it unchanged. Build step concatenates the five source files via
  `PANELS_FILES` + `panels_js()` in
  [lib.rs](../../vxn-1/crates/vxn-ui-web/src/lib.rs) (ESM-stripped,
  `\n;\n`-joined, splice order util/drag → widgets → load-time IIFEs).
- **presetBar dirty-tracking → onMutation hook.** No more
  `send[k] = …` reassignment (`grep -c` = 0). bridge.js gains a
  first-class `window.vxn.onMutation` hook fired by the five
  engine-mutating senders; [preset-bar.js](../../vxn-1/crates/vxn-ui-web/assets/panels/preset-bar.js)
  registers `markDirty` on it.
- **Open/close race resolved.** `open_algo_picker` / `close_algo_picker`
  dropped from `CUSTOM_OPS` in
  [main.js](../../vxn-2/crates/vxn2-ui-web/assets/main.js); op-row wires
  its own overlay buttons with plain bubble-phase listeners (no
  capture-phase + `stopImmediatePropagation`).
- **Tests.** New vitest:
  [algo-data.test.js](../../vxn-2/crates/vxn2-ui-web/assets/__tests__/algo-data.test.js)
  (ALGO_CARRIERS/FB_OPS tables) and
  [ks-graph.test.js](../../vxn-2/crates/vxn2-ui-web/assets/__tests__/ks-graph.test.js)
  (bp/depth-handle drag, sign-bit flips, shape toggles, clamping).
  Green: vxn-2 vitest 35, cargo `vxn2-ui-web` 27; vxn-1 vitest 188,
  cargo `vxn-ui-web` 56. Concatenated vxn-1 production bundle passes
  `node --check` with no `export`/`import` leaks.
