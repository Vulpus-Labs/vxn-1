---
id: "0079"
title: Drag and value-popup lifecycle tests
priority: medium
created: 2026-06-01
epic: E015
---

## Summary

First DOM-dependent tests against the Vitest + jsdom harness from
0077. Cover the two helpers that every primitive depends on:
`wireFaderDrag` ([panels.js:265](../../crates/vxn-ui-web/assets/panels.js#L265))
and `attachValuePop` ([panels.js:314](../../crates/vxn-ui-web/assets/panels.js#L314)).

Locks the drag protocol contract before E016/0082 (generalised
`wireDrag`) reshapes it.

## Acceptance criteria

- [ ] [crates/vxn-ui-web/assets/__tests__/wire-fader-drag.test.js](../../crates/vxn-ui-web/assets/__tests__/wire-fader-drag.test.js)
      covers:
      - `onEnter` fires on `pointerenter`; not during a drag.
      - `onLeave` fires on `pointerleave` when not dragging.
      - `onDown` receives a norm in `[0, 1]` computed from
        `clientY` against the fader's bounding rect; `setPointerCapture`
        is called.
      - `onMove` only fires while `dragging`; receives the same
        norm formula.
      - `onUp` fires on both `pointerup` and `pointercancel`.
      - `onLeave` fires on drag-end-when-not-hovered, not on
        drag-end-while-hovered.
      - The returned getters `isDragging()` / `isHovered()`
        reflect state at every transition.
      - jsdom doesn't implement `setPointerCapture` natively;
        stub it on the test element before wiring.
- [ ] [crates/vxn-ui-web/assets/__tests__/attach-value-pop.test.js](../../crates/vxn-ui-web/assets/__tests__/attach-value-pop.test.js)
      covers:
      - `markEntered` shows the popup with `getLabel()`'s current
        return, except when `host.isDragging()` is true (suppressed).
      - `markLeft` hides only if not dragging.
      - `markGrabbed` always shows (re-anchors at the grab point).
      - `markReleased` hides only if not hovered.
      - `refresh` updates the popup text only when hovered or
        dragging.
      - Tests construct a synthetic `host` object exposing
        `{ isHovered(): bool, isDragging(): bool }` and use the
        module-level `valuePop` indirectly via `bridge.js` import
        (or inject a stub `valuePop` if the test setup makes that
        cleaner).
- [ ] Tests use jsdom-mounted `<div>`s sized via inline
      `getBoundingClientRect` stubs (jsdom returns zero-sized
      rects by default — override on the element prototype before
      each test, or stub the method directly).
- [ ] `npm test` passes; the two new files contribute at least 12
      assertions between them.
- [ ] `cargo test -p vxn-ui-web` passes (unchanged).

## Notes

jsdom quirks worth noting (writing the assertions exposes these):

- No `PointerEvent` constructor in older jsdom; use `new MouseEvent`
  with `pointerType` / `pointerId` set via `Object.defineProperty`,
  or use `dispatchEvent(new Event('pointerdown'))` and attach the
  fields ad-hoc. The Vitest + jsdom 25 combo from 0077 supports
  pointer events; verify on first test.
- `Element.setPointerCapture` and `releasePointerCapture` are
  no-ops on jsdom (it doesn't track capture). Tests stub them
  to record calls.
- `getBoundingClientRect` returns zeros — the test must override
  it (typically via `vi.spyOn(el, 'getBoundingClientRect')` per
  test) so the norm math has a non-degenerate input.

The popup itself (`valuePop`) is a module-level singleton in
`bridge.js`. The test imports `valuePop` and asserts on its DOM
state (the popup's hidden/visible class or `style.display`); no
mocking needed.

If this ticket lands *before* E016/0082, the assertions reference
`wireFaderDrag`. If *after*, they reference the generalised
`wireDrag` and parameterise the `pointerToValue` callback. Either
sequencing works.
