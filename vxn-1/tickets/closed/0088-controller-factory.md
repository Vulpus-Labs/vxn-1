---
id: "0088"
title: Controller-as-factory + primitive registry
priority: high
created: 2026-06-01
epic: E017
---

## Summary

[dispatch.js](../../crates/vxn-ui-web/assets/dispatch.js) currently
exposes its dispatcher as module-level singletons: `model`,
`bindCell`, `rebindAllForLayer`, `applyDimRulesFor`, `init()`.
Promote the surface to a `createController({ params, send, primitives })`
factory. The factory returns the dispatch object; `init()` builds
the controller and wires it to the DOM and the bridge.

Primitives register against their `data-control` kind via the
`primitives` argument — `bindCell` dispatches on a lookup table
instead of the current `switch`. Adding a new primitive then
means handing it to the controller, not editing the switch.

This is the M and C of the MVC the user named in the audit. The
View is the primitive factories themselves; this ticket is what
makes them composable.

## Acceptance criteria

- [ ] [dispatch.js](../../crates/vxn-ui-web/assets/dispatch.js)
      `createController({ params, send, primitives })` returns an
      object with public methods:
      - `dispatch(ev)` — handles a single ViewEvent (today's
        `dispatch` body).
      - `applyViewEvents(arr)` — batch (today's same).
      - `rebindForLayer(layer)` — today's `rebindAllForLayer`.
      - `registerCell(el)` — today's body of the
        `querySelectorAll('[data-control]')` loop body.
      - `bind()` — runs the initial cell registration sweep plus
        the first `rebindForLayer(currentLayer)`.
- [ ] The `primitives` arg shape:
      ```js
      const primitives = {
        fader:           makeFader,
        wave:            makeWave,
        switch:          makeSwitch,
        buttongroup:     makeButtonGroup,
        dropdown:        makeDropdown,
        'header-switch': makeHeaderSwitch,
        'detune-legato': makeDetuneLegato,
      };
      ```
      `bindCell` does `primitives[kind](el, id, desc, opts)`
      instead of a `switch`.
- [ ] All `model.controls` / `model.lastParam` / `model.syncOfRate`
      / etc. become closure-local state inside `createController`.
      No module-level mutables remain in `dispatch.js`.
- [ ] `init()` becomes:
      ```js
      function init() {
        const params = createParamsModel(window.vxn.params,
                                          window.vxn.patchCount);
        const send   = window.vxn.send;
        const ctrl   = createController({ params, send, primitives });
        ctrl.bind();
        window.__vxn.applyViewEvents = (arr) => ctrl.applyViewEvents(arr);
        window.vxn.onViewEvent       = (ev) => ctrl.dispatch(ev);
        // drain _earlyViewEvents into ctrl.dispatch as today.
        send.ready();
      }
      ```
- [ ] The composite primitive's three `addCtl` calls move into
      `bindCell`'s handler for the `detune-legato` kind — the
      factory returns the three update closures plus the three
      ids; the controller registers them. (Pairs with 0089.)
- [ ] [crates/vxn-ui-web/assets/__tests__/controller.test.js](../../crates/vxn-ui-web/assets/__tests__/controller.test.js)
      covers:
      - Construct a controller with the 0080 fixture params, a
        recording-shim send, and a minimal primitives map. Bind
        against a jsdom DOM with two `[data-control]` cells.
      - `dispatch({ kind: 'param_changed', id, plain, norm,
        display })` fans out to the right cell's update.
      - `dispatch({ kind: 'edit_layer_changed', layer: 'lower' })`
        re-resolves cell ids and the next `param_changed` lands
        on the new cell.
      - Dim rules resolve and re-resolve correctly on layer flip.
      - Two controllers can be constructed independently against
        separate DOMs — no shared mutable state.
- [ ] `grep "const model = " dispatch.js` returns zero hits.
- [ ] Manual smoke (ask first): faceplate boots identically;
      every panel works; layer flip, preset load, status flash,
      text-input popup all still work.
- [ ] `npm test` and `cargo test -p vxn-ui-web` pass.

## Notes

`_earlyViewEvents` and `_earlyPresetCorpus` still live in
`bridge.js` — they're the pre-init buffers and need to exist
before the controller does. `init()` drains them after
constructing the controller.

`keysPanel` and `presetBar` and `browserPanel` are still
module-level IIFEs after this ticket — they're singletons by
nature (one per faceplate). 0090 reorganises them into folders
but doesn't make them factory-instantiated. If a future ticket
needs multiple browser panels they can be promoted.

The two-controller test isn't a contrived case — it's the
proof of the lift. If two controllers can run side-by-side in
jsdom, vxn-2 can instantiate its own from the same code.
