---
id: "0076"
title: ESM-ify faceplate JS modules and add splice-loader strip pass
priority: high
created: 2026-06-01
epic: E015
---

## Summary

Make the four faceplate JS files
([bridge.js](../../crates/vxn-ui-web/assets/bridge.js),
[panels.js](../../crates/vxn-ui-web/assets/panels.js),
[browser.js](../../crates/vxn-ui-web/assets/browser.js),
[dispatch.js](../../crates/vxn-ui-web/assets/dispatch.js))
`import`-able from Node by adding `export` statements at the
public surface. The splice loader in
[crates/vxn-ui-web/src/lib.rs](../../crates/vxn-ui-web/src/lib.rs)
strips `^export ` / `^export default ` before concatenating into
the inline `<script>` so wry's eval path stays bit-identical.

This is the prerequisite for every other E015 ticket (and for the
E017 vxn-2 lift). No test code lands here — the deliverable is the
re-import-able source plus a still-working faceplate.

## Acceptance criteria

- [ ] Each of the four JS files declares an `export` block for its
      public surface:
      - `bridge.js` exports `{ valuePop, statusPill, STATUS_PILL_FLASH_MS }`
        plus the helper bootstrap (`_earlyViewEvents`,
        `_earlyPresetCorpus`, `_textInputCallbacks` are still
        module-internal; `window.vxn` / `window.__vxn` assignments
        stay as side-effect bindings).
      - `panels.js` exports `{ presetBar, keysPanel, WAVE_GLYPHS,
        glyphPath, PIXELS_PER_DETENT, KNOB_INDICATOR_TRANSITION_MS,
        TWIN_TOP_CT, SVG_NS, wireFaderDrag, attachValuePop,
        makeFader, makeWave, makeSwitch, makeButtonGroup,
        makeDropdown, makeHeaderSwitch, makeDetuneLegato,
        subdivisionLabel, clampVariant, tgRow, KEY_MODE_NAMES,
        KEY_LAYERS, KEYS_DEFAULT_SPLIT, KEYS_SPLIT_MIN,
        KEYS_SPLIT_MAX, keysNoteName }`.
      - `browser.js` exports `{ browserPanel }`.
      - `dispatch.js` exports `{ init, paramIdByName,
        paramIdByNameAtLayer, variantIdx, isLayeredEl,
        model, addCtl, locateSyncPartners, collectDimRuleSpecs,
        rebuildDimRules, applyDimRulesFor, refreshAllDimRules,
        rateDisplayOverride, bindCell, rebindAllForLayer,
        BUILTIN_DIM_SPECS }`. (Generous on purpose — keeps later
        test tickets free to assert on internals without another
        re-export pass.)
- [ ] [crates/vxn-ui-web/src/lib.rs](../../crates/vxn-ui-web/src/lib.rs)
      `build_faceplate_html` strips `export ` and `export default `
      from the start of each line of every `*_JS` constant before
      splicing. A small `strip_esm_exports(src: &str) -> String`
      helper, called per asset in `build_faceplate_html`. Regex-free:
      a line-iter pass with `line.strip_prefix("export default ")`
      and `line.strip_prefix("export ")` is enough.
- [ ] Substring tests in
      [crates/vxn-ui-web/src/lib.rs](../../crates/vxn-ui-web/src/lib.rs)
      add three assertions: `assembled().contains("export ") == false`
      (the strip ran), the wry-evaluated script still contains
      `function init()` and `window.vxn = {`.
- [ ] Manual smoke (ask first per `ask-before-screen-capture`):
      faceplate boots, all panels render, every primitive responds.
- [ ] `cargo test -p vxn-ui-web` passes.

## Notes

The strip pass is one-way — `export` lines drop their prefix and
become bare declarations, which is exactly what they were before.
`export const X = …` becomes `const X = …`; `export function f(…) {`
becomes `function f(…) {`. No reorder needed because the splice
order already respects declaration order (bridge → browser →
panels → dispatch).

`export default` doesn't appear today; the strip still handles it
for forward compatibility (a future single-default module would
break otherwise).

A `// @ts-check` JSDoc pragma at the top of each file is *not*
required here — it's a future-E015 follow-up if anyone wants type
hints. Out of scope for this ticket.

No `import` statements are added in this ticket. Each file remains
self-sufficient under concatenation (the splice loader still
defines every binding in one shared scope). The `export` annotations
purely document the surface for Node-side `import`s in 0077+.
