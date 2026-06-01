---
id: "0077"
title: Vitest + jsdom devdep, pure-function test suite
priority: high
created: 2026-06-01
epic: E015
---

## Summary

Bring up the Node-side JS test harness under
[crates/vxn-ui-web/assets/](../../crates/vxn-ui-web/assets/) and
seed it with coverage for the pure-function helpers — the ones
that have no DOM dependency and form the lowest-risk first slice.

Depends on 0076 (ESM-ified source). DOM-dependent tests live in
0079; controller / browser invariants live in 0080.

## Acceptance criteria

- [ ] [crates/vxn-ui-web/assets/package.json](../../crates/vxn-ui-web/assets/package.json)
      `{ "type": "module", "private": true, "devDependencies": {
      "vitest": "^2", "jsdom": "^25" }, "scripts": {
      "test": "vitest run", "test:watch": "vitest" } }`.
      No production dependencies; nothing is published.
- [ ] [crates/vxn-ui-web/assets/vitest.config.js](../../crates/vxn-ui-web/assets/vitest.config.js)
      configures the `jsdom` environment and the `__tests__/`
      include pattern. (`jsdom` per-test for the future
      DOM-dependent tests; pure tests don't care which env runs.)
- [ ] [crates/vxn-ui-web/assets/__tests__/](../../crates/vxn-ui-web/assets/__tests__/)
      folder created with the following test files:
      - `clamp-variant.test.js` — at, below-zero, above-len-minus-one,
        non-integer plain, single-variant edge case.
      - `variant-idx.test.js` — known variant, unknown variant
        returns -1, unknown param returns -1, layer routing (upper
        / lower) — uses a fixture `window.vxn.params` object via
        a `beforeEach` global setter.
      - `glyph-path.test.js` — known glyph (`Sine`, `Pulse`) emits
        an `M` + `L` chain; unknown glyph returns `null`; `w` / `h`
        params scale the coordinates.
      - `keys-note-name.test.js` — C0 (n=12), A4 (n=69), edge of
        range, negative octave wrap.
      - `subdivision-label.test.js` — empty table returns empty,
        clamps below 0 and above 1, normal-range lookup.
      - `move-targets.test.js` — current folder excluded, root
        included unless `currentName === null`, alpha sort
        case-insensitive, empty corpus.
      - `folder-value.test.js` + `folder-options.test.js` — root
        sentinel handling, sort order, dedup against the virtual
        root.
- [ ] Every test file uses `import { … } from '../<file>.js'` and
      mocks `window.vxn` via `globalThis` setup where needed.
- [ ] [crates/vxn-ui-web/assets/README.md](../../crates/vxn-ui-web/assets/README.md)
      created. One paragraph: how the splice loader works, how
      to run `npm test`, where new tests live, the lift contract
      (forward note for E017).
- [ ] [.gitignore](../../.gitignore) excludes `node_modules/`
      anywhere under `crates/vxn-ui-web/`.
- [ ] `npm install && npm test` under
      [crates/vxn-ui-web/assets/](../../crates/vxn-ui-web/assets/)
      passes on a fresh checkout.
- [ ] `cargo test -p vxn-ui-web` still passes — the wry-side
      assembled script is unchanged by anything in this ticket.

## Notes

Vitest is preferred over Jest because it's ESM-first (we just
ESM-ified) and faster to set up. Either would work; the
recommendation stands unless the rest of the repo grows a Jest
dependency first.

The first suite is deliberately DOM-free so the harness lands
without arguing about how to fake pointer events. 0079 adds
those.

Fixture params tables live inline in each test that needs one —
don't centralise yet. When 0080 adds dispatch tests, a shared
`fixtures/params.js` becomes worth extracting.

`@vitest/coverage-v8` is not added here. If anyone wants
coverage numbers it's a one-line `package.json` addition; for
now the suite is small enough that the test file list *is* the
coverage report.
