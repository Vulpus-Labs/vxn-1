---
id: "0086"
title: Inject `send` into primitive factories (no `window.vxn.send` reads in panels.js)
priority: medium
created: 2026-06-01
epic: E017
---

## Summary

The 0075 audit's finding N1: every primitive factory in
[panels.js](../../crates/vxn-ui-web/assets/panels.js) reaches into
the global `window.vxn.send` to post IPC. That couples the
primitives to the bridge global and blocks the vxn-2 lift, where
the same primitives need to wire up against vxn-2's own bridge
instance.

Inject `send` as part of each factory's opts argument. The
controller (today: `bindCell` in
[dispatch.js](../../crates/vxn-ui-web/assets/dispatch.js)) captures
`window.vxn.send` once and passes it through. The only
`window.vxn.send` read in `panels.js` after this ticket is in the
preset bar and keys panel — both of which are not primitive
factories but bound widgets; they get the same treatment for
consistency.

## Acceptance criteria

- [ ] Every primitive factory in `panels.js` gains a `send` field
      in its opts object:
      - `makeFader(el, id, desc, { send, displayOverride })`.
      - `makeWave(el, id, desc, { send })`.
      - `makeSwitch(el, id, desc, { send })`.
      - `makeButtonGroup(el, id, desc, { send })`.
      - `makeDropdown(el, id, desc, { send })`.
      - `makeHeaderSwitch(el, id, desc, { send })`.
      - `makeDetuneLegato(el, ids, descs, modeName, layer, { send })`.
- [ ] Inside each factory, every `window.vxn.send.X(...)` becomes
      `send.X(...)`.
- [ ] [dispatch.js `bindCell`](../../crates/vxn-ui-web/assets/dispatch.js#L210)
      passes `{ send: window.vxn.send, ... }` to each factory. The
      capture is one read at the top of `init()` for now; 0088
      promotes it to the controller's constructor argument.
- [ ] `presetBar` ([panels.js:13](../../crates/vxn-ui-web/assets/panels.js#L13))
      and `keysPanel` ([panels.js:78](../../crates/vxn-ui-web/assets/panels.js#L78))
      gain a `send` parameter via a small wrapping factory (`createPresetBar(send)`,
      `createKeysPanel(send)`) — or, if cleaner, accept `send` via
      their existing IIFE pattern by promoting the IIFE to a
      named function. The latter pairs with 0088.
- [ ] `grep window.vxn.send` in
      [panels.js](../../crates/vxn-ui-web/assets/panels.js) returns
      zero hits. Hits remain in `bridge.js` (definition) and
      `dispatch.js` / `browser.js` (still pending in this epic;
      0090 sweeps them).
- [ ] [crates/vxn-ui-web/assets/__tests__/primitive-send-injection.test.js](../../crates/vxn-ui-web/assets/__tests__/primitive-send-injection.test.js)
      covers (one assertion per primitive):
      - Construct each factory with a recording-shim `send` and
        a jsdom-mounted target.
      - Trigger the relevant interaction (click on switch,
        `pointerdown` on fader, glyph click on wave knob, etc).
      - Assert the shim received the expected call sequence — no
        reads from `window.vxn.send` (which is `undefined` in
        the test environment).
- [ ] Manual smoke (ask first): every primitive still writes back
      through the bridge in a host.
- [ ] `npm test` and `cargo test -p vxn-ui-web` pass.

## Notes

The composite (`makeDetuneLegato`) is the trickiest because of
its three internal `send` calls (begin/end gesture, set_param,
discrete for the legato toggle and Twin clamp). All three become
`send.X` calls; semantics unchanged.

Double-click reset handlers (`el.addEventListener('dblclick', ...)`)
inside `bindCell` already call `window.vxn.send.discrete(id, desc.default)`;
those become `send.discrete(...)` once `bindCell` captures `send`
locally (which `init()` already does after this ticket).

If 0088 lands first this ticket is partially subsumed — the
controller already holds `send`. Leave 0086 open and complete it
as a sweep ticket then; the work is the same.
