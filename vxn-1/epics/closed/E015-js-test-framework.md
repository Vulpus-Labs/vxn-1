---
id: E015
title: Faceplate JS unit-test framework — stabilising net for E016/E017
status: open
created: 2026-06-01
---

## Goal

Stand up a behavioural test net for the four faceplate JS modules
([bridge.js](../../crates/vxn-ui-web/assets/bridge.js),
[panels.js](../../crates/vxn-ui-web/assets/panels.js),
[browser.js](../../crates/vxn-ui-web/assets/browser.js),
[dispatch.js](../../crates/vxn-ui-web/assets/dispatch.js)) before the
post-E014 boundary cleanup (E016) and the reusable-primitive lift
(E017) touch the surface. The test framework is the stabilising
contract for both follow-on epics — each ticket in E016/E017 lands
its refactor *and* the tests that cover the surface it exposes.

Two-step bootstrap:

1. Make the JS files `import`-able from Node — they're currently
   concatenated by the splice loader at HTML build time, so `export`
   syntax has nowhere to land. A one-line strip pass in the splice
   loader makes ESM syntax safe for both worlds.
2. Bring up [Vitest](https://vitest.dev/) + [jsdom](https://github.com/jsdom/jsdom)
   under `crates/vxn-ui-web/assets/` as devDependencies (no
   production Node dep). The first ticket ports the pure-function
   helpers (`clampVariant`, `variantIdx`, `glyphPath`, `keysNoteName`,
   `subdivisionLabel`, `moveTargets`, `folderOptions`, `folderValue`)
   under test; subsequent tickets add DOM-dependent coverage
   (`wireFaderDrag`, `attachValuePop`, dim-rule resolution, browser
   invariants).

## Background

The 0075 close-out audit (E014) noted: the substring suite in
`vxn-ui-web::tests::faceplate_*` catches gross regressions (typed
sender names exist, opcode strings match `UiEvent`, status-pill
markup present) but verifies *no behaviour*. With the four files at
~155 / 445 / 780 / 925 lines, behavioural coverage is the next
correctness lever. E016 and E017 will both reshape primitive
boundaries — without a behavioural net, each refactor lands blind.

## In scope

- ESM-ify the JS source (`export` statements at module level).
- Splice loader: strip `^export ` (and `^export default `) before
  concatenating into the inline `<script>`. Wry's WebView keeps
  eating the same single-block script.
- Vitest + jsdom dev environment under `crates/vxn-ui-web/assets/`
  (Node 20+, npm scripts only — no bundler in production).
- A growing test suite, seeded with pure-function coverage and
  expanded by each downstream ticket.
- Wire the JS suite into `cargo test -p vxn-ui-web` (cfg-gated /
  `#[ignore]` when Node isn't on PATH) **or** a dedicated CI job
  gated independently — pick one in 0078.

## Out of scope

- TypeScript / type checking.
- A production bundler (esbuild, Rollup). The splice loader stays
  the production path.
- Coverage of WKWebView-specific quirks (drag-on-Safari, native
  popup menu, OS native text-input window) — manual smoke covers
  these.
- Coverage of CSS layout. The substring suite remains the existing
  pin on `--panel-h`, `--fader-h`, etc.
- Visual regression testing.

## Phasing

Tickets land in order; later tickets depend on the framework
established by earlier ones.

1. **0076** ESM-ify the four modules + splice-loader strip pass.
   Foundation — no test code yet. Verifies the wry-side eval still
   works bit-identically.
2. **0077** Vitest + jsdom + first pure-function suite.
   Establishes test harness, npm scripts, and the first ~8
   behavioural assertions.
3. **0078** Wire JS tests into the cargo test path (or CI).
   Pick: `#[test] fn js_suite_passes` shelling `npm test`, or
   separate GH Actions job. Decision locked in this ticket.
4. **0079** DOM-dependent primitive coverage: `wireFaderDrag` and
   `attachValuePop` lifecycle tests. First jsdom-based DOM
   assertions; serves as a template for downstream tickets.
5. **0080** Dispatch + browser invariants: dim-rule resolution
   against a fixture params table, `browserPanel.followPath` /
   `setCorpus` / `moveTargets` behaviour. Locks the dispatcher's
   contract before E016 touches it.

Each E016 / E017 ticket adds its own tests as part of acceptance,
not as a separate ticket — the net grows with the refactor surface.

## Tickets

- [ ] [0076 — ESM-ify JS modules and add splice-loader strip](../../tickets/open/0076-esm-modules-splice-strip.md)
- [ ] [0077 — Vitest + jsdom + pure-function suite](../../tickets/open/0077-vitest-jsdom-bootstrap.md)
- [ ] [0078 — Wire JS suite into cargo test or CI](../../tickets/open/0078-wire-js-tests-into-ci.md)
- [ ] [0079 — Drag and value-popup lifecycle tests](../../tickets/open/0079-drag-popup-lifecycle-tests.md)
- [ ] [0080 — Dispatch and browser invariant tests](../../tickets/open/0080-dispatch-browser-invariants.md)

## Acceptance

- `npm test` under `crates/vxn-ui-web/assets/` passes on a clean
  checkout (Node 20+ installed).
- `cargo test -p vxn-ui-web` either drives the JS suite (0078
  option A) or remains the gross-regression smoke alongside a
  documented CI gate (0078 option B).
- The wry-evaluated faceplate behaves bit-identically to pre-E015.
  Manual smoke (ask first per `ask-before-screen-capture`) covers
  the live host.
- Every helper covered in 0077 / 0079 / 0080 has at least one
  positive and one boundary assertion.
- The README under `crates/vxn-ui-web/assets/` documents the test
  command and the expected dev workflow (one paragraph).
