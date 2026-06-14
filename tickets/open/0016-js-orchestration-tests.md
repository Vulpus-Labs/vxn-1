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
