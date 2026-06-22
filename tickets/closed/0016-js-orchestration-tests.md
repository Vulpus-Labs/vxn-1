---
id: "0016"
product: vxn-1
title: Direct vitest coverage for dispatch.js orchestration
priority: medium
created: 2026-06-10
epic: E011
---

## Summary

The vitest suite (143 cases) covers primitives and the
browser panel well, but the orchestration layer in
`dispatch.js` — the most complex JS in the faceplate — has
zero direct tests. Today its only safety net is Rust-side
substring assertions on the spliced HTML, which check the
identifiers exist, not that the logic works. A refactor
breaks layer switching silently.

Untested surface:

- `rebindAllForLayer` (`dispatch.js:349`, ~49 lines): clears
  `model.controls`, re-resolves param ids at the new layer,
  calls `locateSyncPartners` + `rebuildDimRules`, reseeds
  every cell from `lastParam`.
- The four override factories: `rateDisplayOverride`,
  `cutoffInteractionOverride`, `cutoffNormOverride`,
  `cutoffDisplayOverride` (`dispatch.js:240-276`) — sync
  subdivision display and note-quantised cutoff mapping.
- `locateSyncPartners` — populates `model.syncOfRate` /
  `rateOfSync` / `tunedOfCutoff`.
- End-to-end `init()` → `applyViewEvents` fan-out for
  `param_changed`, including sync-partner refresh and dim
  re-application.
- `presetBar` overwrite-save flow (`panels.js:103-119`).

## Acceptance criteria

- [ ] `rebindAllForLayer` test: mount fixture DOM (existing
      `fixtures/params.js` + setup.js pattern), bind at
      Upper, flip to Lower, assert every cell's bound id
      shifted by `patchCount`, values reseeded from
      `lastParam`, dim rules reapplied.
- [ ] Override factory tests: cutoff-tuned ON maps drag norm
      → quantised Hz and displays note name; OFF returns
      null (passthrough). Rate override shows subdivision
      label when sync partner active, numeric otherwise.
- [ ] `locateSyncPartners` test: maps populated correctly
      from fixture params; unknown names tolerated.
- [ ] One integration test: `init()` then synthetic
      `applyViewEvents` batch; assert DOM reflects values,
      a `param_changed` on a sync toggle refreshes its rate
      partner's display.
- [ ] `presetBar` Save (overwrite) test alongside the
      existing Save-As coverage.
- [ ] Suite still passes via `VXN_JS_TESTS=1 cargo test -p
      vxn-ui-web` and in CI (0116).

## Notes

No production-code changes in this ticket — if testing
forces a refactor (e.g. exporting an internal), keep it
mechanical and behaviour-preserving; cleanup belongs to
0020, which depends on this ticket precisely so the tests
pin behaviour first.

The `cutoffTuned*` helpers in `panels.js` are already
unit-tested; what is missing is the factory wiring in
dispatch.js that decides when they apply.

## Close-out (2026-06-22)

New direct vitest coverage for the dispatch orchestration layer; no
production-code changes — only test files plus a fixture extension.

- **New suite** `__tests__/dispatch-orchestration.test.js` (9 cases).
  dispatch.js imports nothing (concat-time globals at splice), so the
  cross-module symbols — `makeFader`/`makeSwitch`/…, `subdivisionLabel`,
  the `cutoffTuned*` helpers, `keysPanel`/`presetBar`/`browserPanel`,
  and bridge's `_earlyViewEvents`/`_textInputCallbacks` — are stubbed on
  `globalThis`, exactly as the splice would define them.
  - `locateSyncPartners`: rate↔sync + cutoff↔tuned maps at upper, per-
    patch ids shift `+patchCount` at lower, globals stay put, missing
    params skipped without throwing.
  - `rateDisplayOverride`: null with no partner; returns the subdivision
    label only while the partner sync is on.
  - cutoff overrides (`cutoffDisplayOverride`/`cutoffNormOverride`/
    `cutoffInteractionOverride`): null off a cutoff fader; route through
    the tuned helpers only while Tuned is on, else passthrough (null).
  - `rebindAllForLayer`: every layered cell re-binds to the new layer
    ids (`5,7` → `15,17`), partners re-resolve, the freshly-bound cell is
    reseeded from `model.lastParam`.
  - `init()` → `applyViewEvents`: cells bind, a `param_changed` drives
    its ctl, and a sync-toggle echo refreshes its rate partner's display.
- **Fixture** `fixtures/params.js` gained the sync/cutoff params the
  above needs (`lfo_rate`/`lfo_sync`, `cutoff`/`cutoff_tuned` per-patch;
  `lfo2_rate`/`lfo2_sync`, `delay_time`/`delay_sync` global). Additive —
  existing ids/names unchanged, so `dim-rules.test.js` et al stay green.
- **presetBar overwrite-Save** (`__tests__/preset-bar.test.js`, 3 new
  cases): gated disabled until dirty AND source is a user preset;
  `savePreset(name, folder)` on a dirty user preset then re-disables;
  refuses when `folderForUserPath` is undefined (no silent fork). Loaded
  via a keep-bridge helper so panels' dirty-wrap on `send.setParam`
  survives (the recorder swap would lose it).
- Green via `npx vitest run` (168 cases) and the gated Rust path
  `VXN_JS_TESTS=1 cargo test -p vxn-ui-web` (56 cases incl.
  `js_suite_passes`), which is what CI (0116) runs.
