---
id: "0090"
title: Source-tree reorganisation into primitives/ controller/ browser/ bridge/
priority: medium
created: 2026-06-01
epic: E017
---

## Summary

Reorganise [crates/vxn-ui-web/assets/](../../crates/vxn-ui-web/assets/)
from four top-level JS files into four folders matching the MVC
boundary established by 0086–0089:

```
assets/
├── faceplate.html
├── faceplate.css
├── bridge/
│   ├── ipc.js              // _post, send namespace, _earlyViewEvents, applyViewEvents stub
│   ├── value-pop.js        // valuePop singleton
│   ├── status-pill.js      // statusPill singleton
│   ├── text-input.js       // promptText + _textInputCallbacks
│   └── index.js            // re-exports
├── controller/
│   ├── params.js           // createParamsModel (from 0087)
│   ├── controller.js       // createController (from 0088)
│   ├── dim-rules.js        // collectDimRuleSpecs, rebuildDimRules,
│   │                       //   applyDimRulesFor, BUILTIN_DIM_SPECS
│   ├── sync-partners.js    // locateSyncPartners, rateDisplayOverride
│   ├── init.js             // init(), wiring of everything
│   └── index.js            // re-exports
├── primitives/
│   ├── fader.js            // makeFader, paintFader (from 0083), wireDrag (from 0082)
│   ├── wave.js             // makeWave, WAVE_GLYPHS, glyphPath, SVG_NS
│   ├── switch.js           // makeSwitch
│   ├── button-group.js     // makeButtonGroup
│   ├── dropdown.js         // makeDropdown
│   ├── header-switch.js    // makeHeaderSwitch
│   ├── detune-legato.js    // makeDetuneLegato
│   ├── tg-row.js           // tgRow (from 0085)
│   ├── attach-value-pop.js // attachValuePop
│   ├── constants.js        // PIXELS_PER_DETENT, KNOB_INDICATOR_TRANSITION_MS,
│   │                       //   TWIN_TOP_CT, SVG_NS, STATUS_PILL_FLASH_MS
│   └── index.js            // primitives registry + re-exports
├── browser/
│   ├── panel.js            // browserPanel IIFE (or factory after 0091)
│   ├── modal.js            // mountModal, openConfirmModal,
│   │                       //   openSaveAsModal (if N7 lift triggered)
│   ├── corpus.js           // setCorpus, folderExists, folderLabel,
│   │                       //   collectSearchHits, findGroup, moveTargets,
│   │                       //   folderOptions, folderValue
│   ├── dnd.js              // wirePresetDragSource, drag state
│   └── index.js
└── keys/
    ├── keys-panel.js       // keysPanel + KEY_MODE_NAMES, KEY_LAYERS,
    │                       //   KEYS_DEFAULT_SPLIT, KEYS_SPLIT_MIN/MAX,
    │                       //   keysNoteName
    └── index.js
```

(Sketch — final layout decided in this ticket; minor adjustments
welcome if a different boundary reads better. The principle is
one concern per file, primitives importable as a directory.)

## Acceptance criteria

- [ ] Files reorganised per the layout above (or an agreed
      variant). Each file contains its public exports plus
      whatever it needs to satisfy them; no cross-file mutable
      state.
- [ ] [crates/vxn-ui-web/src/lib.rs](../../crates/vxn-ui-web/src/lib.rs)
      `include_str!` paths and the splice-loader's
      concatenation order update to walk the new tree. Order:
      `bridge/` → `controller/params.js` → `controller/dim-rules.js`
      → `controller/sync-partners.js` → `primitives/` (constants
      → helpers → factories) → `keys/` → `browser/` →
      `controller/controller.js` → `controller/init.js`.
- [ ] Each folder has an `index.js` that re-exports the folder's
      public surface for cleanly importing in tests:
      `import { makeFader } from '../primitives/index.js'`.
- [ ] Substring tests update to the new file boundaries — every
      `assembled().contains(...)` assertion that was on the
      old four files still holds against the assembled HTML
      (the splice output is bit-identical except for ordering
      tweaks needed to satisfy declaration order).
- [ ] [crates/vxn-ui-web/assets/__tests__/](../../crates/vxn-ui-web/assets/__tests__/)
      imports update to the new paths. Existing assertions are
      unchanged; only the `import` lines move.
- [ ] [crates/vxn-ui-web/assets/README.md](../../crates/vxn-ui-web/assets/README.md)
      updated with the new tree + the lift contract: "vxn-2:
      import `primitives/`, `browser/`, `keys/` as source; write
      your own `bridge/ipc.js` and `controller/init.js` against
      your descriptor table."
- [ ] No file under `assets/` exceeds ~200 lines (sanity check
      — if one does, it's a code smell that file is doing too
      much; raise it in close-out).
- [ ] Manual smoke (ask first): faceplate boots identically.
- [ ] `npm test` and `cargo test -p vxn-ui-web` pass.

## Notes

This is a large mechanical change. The risk lives entirely in
splice-loader ordering and in the substring tests — if both pass
and manual smoke shows zero regression, the move is sound.

The four top-level files (`bridge.js`, `panels.js`,
`browser.js`, `dispatch.js`) are deleted at the end. The
`__BRIDGE_JS__` etc. placeholders in `faceplate.html` either:
- (a) collapse to one `__JS__` placeholder that the loader
  fills with the concatenated tree, or
- (b) each folder gets its own placeholder (`__BRIDGE__`,
  `__CONTROLLER__`, etc.) for readability of the HTML.
Pick (a) — the HTML doesn't care about boundaries, and the
loader is the only thing that does.

If folder boundaries don't read well after the move (e.g.
`primitives/` ends up importing `bridge/value-pop.js`,
implying the boundary leaks), revisit before close — the lift
contract depends on `primitives/` being self-contained against
just `bridge/`'s injected interface.
