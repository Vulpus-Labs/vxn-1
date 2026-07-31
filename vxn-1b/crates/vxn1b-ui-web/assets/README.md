# vxn-ui-web faceplate assets

This directory holds the four JS modules — `bridge.js`, `browser.js`,
`panels.js`, `dispatch.js` — plus the HTML scaffold and the CSS. The Rust
crate `vxn-ui-web` `include_str!`s each one, strips ESM `export` /
`export default` line prefixes (`strip_esm_exports` in
[../src/lib.rs](../src/lib.rs)), and splices them into a single inline
`<script>` inside [faceplate.html](./faceplate.html). The wry WebView
evaluates that script as a classic script; the splice order
(bridge → browser → panels → dispatch) is what lets each module reference
the previous one's top-level bindings without an `import`.

## Tests (E015)

The `__tests__/` folder hosts the Node-side behavioural suite. The four
JS modules carry `export` markers so Vitest can `import` them; the splice
loader peels those markers per line at editor-open time so the wry path
stays bit-identical. The top-level IIFEs in `panels.js` (`presetBar`) and
`browser.js` (`browserPanel`) early-return with shape-matching stubs when
the faceplate DOM elements are absent — that's the headless ESM path; in
the real wry page the elements exist and the full wiring runs.

Run from this directory:

```bash
npm install
npm test          # one-shot
npm run test:watch
```

The dev deps (Vitest + jsdom) are devDependencies only. Nothing under
`crates/vxn-ui-web/` ships JS at runtime; the published artefact is the
Rust crate that embeds the asset strings.

The same suite is reachable through `cargo test` via an env-gated
`#[test]` (E015 / 0078):

```bash
VXN_JS_TESTS=1 cargo test -p vxn-ui-web
```

Without `VXN_JS_TESTS`, the `js_suite_passes` test no-ops with a "skip"
note on stderr so a Rust-only developer doesn't need Node on PATH. CI sets
the var when it gates the suite. The Vitest invocation reads `npm test
--silent` from this directory.

The seeded suite covers the pure-function helpers (`clampVariant`,
`variantIdx`, `glyphPath`, `keysNoteName`, `subdivisionLabel`,
`moveTargets`, `folderValue`, `folderOptions`). DOM-dependent helpers
(`wireFaderDrag`, `attachValuePop`, dim-rule resolution, browser
invariants) land per-ticket as the E015 epic progresses
([epics/open/E015-js-test-framework.md](../../../epics/open/E015-js-test-framework.md)).

## Future: the E017 lift

Each `export` block doubles as the lift contract for the vxn-2 reusable
primitives epic (E017). When a primitive moves to its standalone module,
its current export here defines the public surface the new module must
preserve.
