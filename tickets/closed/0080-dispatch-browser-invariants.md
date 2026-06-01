---
id: "0080"
title: Dispatch and browser invariant tests
priority: medium
created: 2026-06-01
epic: E015
---

## Summary

Last of the E015 seed coverage: dim-rule resolution from
[dispatch.js](../../crates/vxn-ui-web/assets/dispatch.js) and the
browser panel's state-preservation invariants from
[browser.js](../../crates/vxn-ui-web/assets/browser.js). These are
the surfaces E016 and E017 will most disturb, so the assertions
serve as the contract those epics restore.

## Acceptance criteria

- [ ] [crates/vxn-ui-web/assets/fixtures/params.js](../../crates/vxn-ui-web/assets/fixtures/params.js)
      a small fixture exporter that returns a minimal
      `window.vxn.params`-shaped object covering one per-patch enum
      (per-patch `assign_mode` with variants Poly/Unison/Solo/Twin),
      one global enum (`filter_mode` with Lowpass/Highpass/Bandpass/Notch),
      one bool (`lfo1_free_run`), one continuous (`lfo1_delay_time`),
      one continuous (`lfo1_fade`), one continuous (`filter_slope`),
      and the matching upper/lower id pairings (patchCount=10 is
      enough). Used by 0080's tests and any future
      dispatch-related test.
- [ ] [crates/vxn-ui-web/assets/__tests__/dim-rules.test.js](../../crates/vxn-ui-web/assets/__tests__/dim-rules.test.js)
      covers:
      - `collectDimRuleSpecs` picks up `data-dim-when-src-off`
        and `data-dim-unless-fm` attributes from a jsdom-mounted
        DOM and stores spec shape correctly.
      - `rebuildDimRules` resolves spec watch names to current-
        layer ids; produces predicates that match expected plain
        values; produces the right number of rules from the
        built-in specs (`lfo1_free_run`, `filter_mode`).
      - `applyDimRulesFor` toggles `.dimmed` on the right targets
        and leaves others alone.
      - A layer flip (upper → lower) re-resolves ids correctly
        and the predicates still fire for the cached
        `lastParam`s.
- [ ] [crates/vxn-ui-web/assets/__tests__/browser-invariants.test.js](../../crates/vxn-ui-web/assets/__tests__/browser-invariants.test.js)
      covers (every test mounts a fixture DOM with the browser
      panel's element ids and calls into `browserPanel`'s exposed
      API):
      - `setCorpus` with a corpus that omits the previously-
        selected folder collapses selection to user root.
      - `setCorpus` mid-search preserves the query.
      - `followPath` selects the new folder, clears search, and
        scrolls the target row into view (assert `scrollIntoView`
        called on the right row via a spy).
      - `setCurrentSource(null)` clears the `current` class on
        every preset row.
      - `moveTargets` excludes the current folder, includes root
        unless `currentName === null`, alpha sort case-insensitive.
      - `openSaveAs` invoked with the empty name disables the OK
        button until a name is entered via `promptText`.
- [ ] Tests stub `window.vxn.send` with a recording shim; assert
      on the recorded call sequence after each interaction.
- [ ] Tests stub `window.vxn.promptText` with a synchronous
      shim that immediately invokes the callback with a test value.
- [ ] `npm test` passes; the two new files contribute at least
      18 assertions between them.
- [ ] `cargo test -p vxn-ui-web` passes (unchanged).

## Notes

The fixture params table is deliberately small. Real `window.vxn.params`
has ~150 entries; the dispatcher logic is the same at 10. If a
specific test needs a larger fixture, build it ad-hoc — don't
grow the shared fixture beyond what dispatch needs.

`browserPanel` is a module-level IIFE today. Importing it in a
test runs the IIFE — which means the test DOM must contain every
id `browserPanel` querySelects (`browser-panel`, `browser-backdrop`,
etc.) *before* the `import` happens. Use Vitest's `beforeEach`
with `document.body.innerHTML = '…'` and a *fresh* dynamic import
per test (`vi.resetModules() + await import('../browser.js')`).
This is awkward — E017/0088 will fix it by making the panel a
factory.

`scrollIntoView` isn't implemented by jsdom; stub on
`Element.prototype` before each test or skip the spy and assert
on selection state instead.

The dim-rule tests should *not* exercise the BUILTIN_DIM_SPECS
inline — call `rebuildDimRules('upper')` and assert on the
resulting `model.dimRules` length and `.predicate` results. The
spec list itself is documented in code; the tests assert behaviour.
