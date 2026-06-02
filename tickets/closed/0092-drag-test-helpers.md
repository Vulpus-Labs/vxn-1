---
id: "0092"
title: Shared drag-test helpers and wireDrag/wireFaderDrag dedup
priority: low
created: 2026-06-01
epic: E017
---

## Summary

The two pointer-drag suites
([wire-drag.test.js](../../crates/vxn-ui-web/assets/__tests__/wire-drag.test.js),
[wire-fader-drag.test.js](../../crates/vxn-ui-web/assets/__tests__/wire-fader-drag.test.js))
each redefine an identical `pointerEvt(type, opts)` factory and a
near-identical `makeEl` / `makeFader` mount. They also overlap on
the pointer-capture lifecycle and hover-suppression-during-drag
contract, even though `wireFaderDrag` is a thin wrapper around
`wireDrag` ([panels.js:265](../../crates/vxn-ui-web/assets/panels.js#L265))
and the shared contract is already covered by the generalised
suite.

Lift the helpers into `crates/vxn-ui-web/assets/__tests__/_helpers.js`
and trim the duplicated coverage. Also lift the `installVxn` /
`loadPanel` shape from
[browser-invariants.test.js](../../crates/vxn-ui-web/assets/__tests__/browser-invariants.test.js)
so 0093's new browser-suite files can import it.

## Acceptance criteria

- [ ] [crates/vxn-ui-web/assets/__tests__/_helpers.js](../../crates/vxn-ui-web/assets/__tests__/_helpers.js)
      exports:
      - `pointerEvt(type, { clientX, clientY, pointerId } = {})`
        — `MouseEvent` with `pointerId` / `clientX` / `clientY`
        grafted on (jsdom omits `PointerEvent`).
      - `mountEl()` — creates a `<div>`, appends to body, stubs
        `setPointerCapture` / `releasePointerCapture` with
        `vi.fn()`; returns the element.
      - `mountFader({ top, height } = { top: 100, height: 200 })`
        — wraps `mountEl()` and spies `getBoundingClientRect` to
        return the supplied window; returns the element.
      - `installVxn(opcodes, { promptValue = null } = {})` —
        seeds `window.vxn.send` with recording shims for every
        opcode name in `opcodes`; seeds `window.vxn.promptText`
        with a synchronous `(_, _, cb) => cb(promptValue)` shim.
        Returns `{ send, sendCalls }`.
      - `loadBrowserPanel()` — `vi.resetModules()` then dynamic
        import of `../browser.js`; returns the imported
        `browserPanel`.
- [ ] The filename is `_helpers.js` (underscore prefix) to keep
      Vitest's default `*.test.js` discovery from picking it up.
      Verify by running `npm test` and confirming the test count
      matches the prior run minus the deleted assertions.
- [ ] [wire-drag.test.js](../../crates/vxn-ui-web/assets/__tests__/wire-drag.test.js)
      and
      [wire-fader-drag.test.js](../../crates/vxn-ui-web/assets/__tests__/wire-fader-drag.test.js)
      replace their inline `pointerEvt` / `makeEl` / `makeFader`
      definitions with `import { … } from './_helpers.js'`.
- [ ] [wire-fader-drag.test.js](../../crates/vxn-ui-web/assets/__tests__/wire-fader-drag.test.js)
      drops the now-redundant assertions:
      - The "captures the pointer" assertion in the "onDown
        captures the pointer and reports norm…" test — keep the
        norm math, drop the `setPointerCapture` check (covered
        by `wire-drag.test.js`'s "pointer capture + dragging
        class lifecycle" test).
      - The "pointer capture release on pointerup" assertion in
        the "pointerup ends the drag, releases capture, and
        fires onUp" test — keep the `onUp` and `isDragging()`
        checks, drop the `releasePointerCapture` check.
- [ ] [browser-invariants.test.js](../../crates/vxn-ui-web/assets/__tests__/browser-invariants.test.js)
      replaces its inline `installVxn` / `loadPanel` /
      `browserDOM` with imports from `_helpers.js` (move
      `browserDOM` too — 0093 will reuse it).
- [ ] `npm test` passes; coverage unchanged (only the duplicated
      lifecycle assertions are removed, contract is still
      asserted in `wire-drag.test.js`).
- [ ] `cargo test -p vxn-ui-web` passes.

## Notes

`wireFaderDrag` is the fader-shaped specialisation of `wireDrag`
post-0082 — once the wrapper contract is tested in `wire-drag.test.js`,
the fader suite only needs to lock down what `wireFaderDrag`
adds on top: the bounding-rect → norm math and the [0,1] clamp.
Everything else is `wireDrag`'s contract.

`_helpers.js` is not a behaviour change — pure file
reorganisation. Land in one commit before 0093 to keep the
diff for the new browser tests focused on actual assertions.

`browserDOM` is currently a nested function in
`browser-invariants.test.js` — it returns the fixture HTML
string. Lift verbatim; 0093's new suites use the same shape.

The `installVxn` lift also gives a place to grow the opcode list
without re-listing it in every browser-suite file. Current set
from `browser-invariants.test.js` already covers all 10 opcodes
the browser panel uses; new suites should pass a subset relevant
to what they exercise.
