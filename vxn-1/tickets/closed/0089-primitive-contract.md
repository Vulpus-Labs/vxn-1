---
id: "0089"
title: Unify primitive return shape; lift `addCtl` out of composite
priority: medium
created: 2026-06-01
epic: E017
---

## Summary

Every primitive factory should return `{ update, dispose }` and
have *no* side effects on the controller's mutable state. Today
six primitives meet that contract (return `{ update }`); only
`makeDetuneLegato` calls `addCtl` three times from inside its
factory body
([panels.js:247–249](../../crates/vxn-ui-web/assets/panels.js#L247-L249)).

Lift those three calls into the controller's `detune-legato`
handler in `bindCell` (after 0088's primitive registry exists).
The factory returns `{ ids: [detuneId, legatoId, modeId],
detuneUpdate, legatoUpdate, modeUpdate }` (or a shape that
collapses to three `{ id, update }` pairs); the controller
registers each.

While we're touching every factory, add a `dispose()` method
that removes event listeners — currently the only "dispose"
behaviour is `bindCell` clearing `el.innerHTML` on layer flip,
which leaks listeners attached to the old children's pointer
events (browsers usually GC them with the DOM nodes, but the
contract should be explicit).

## Acceptance criteria

- [ ] Every primitive factory returns `{ update, dispose }`.
      `dispose()` removes any listener attached to elements
      *outside* the factory's `el` (today: none — every listener
      is on `el` or its descendants, which the controller's
      `el.innerHTML = ''` reclaims). For now `dispose` is a
      no-op for primitives that only listen on `el`; explicit
      contract surface for the future.
- [ ] `makeDetuneLegato` returns `{ update, dispose, ids,
      updates }` where:
      - `ids = { detune, legato, mode }`.
      - `updates = { detune: (p, n, d) => …, legato: (p) => …,
        mode: (p) => … }`.
      The composite's three update closures are exposed as
      methods on the returned object; the factory no longer
      calls `addCtl`.
- [ ] [dispatch.js](../../crates/vxn-ui-web/assets/dispatch.js)
      `bindCell` (inside the controller from 0088) handles the
      `detune-legato` kind by registering three update closures
      against the three ids using the composite's returned
      `ids` and `updates`. Other primitive kinds register a
      single update against their one id (unchanged from 0088).
- [ ] `grep "addCtl" panels.js` returns zero hits.
- [ ] `grep "function " panels.js | grep "make"` — every `make*`
      function's return statement contains `update` and `dispose`
      keys. (Manual visual check; not a grep assertion.)
- [ ] [crates/vxn-ui-web/assets/__tests__/primitive-contract.test.js](../../crates/vxn-ui-web/assets/__tests__/primitive-contract.test.js)
      covers:
      - Every factory's returned object has `update` and
        `dispose` fields (compile-time-ish contract check via
        introspection).
      - `dispose()` is safe to call on every primitive; calling
        it twice doesn't throw.
      - The composite's `ids` and `updates` shape — the
        controller test in 0088 already covers the
        registration; this test covers the composite's surface
        in isolation.
- [ ] Manual smoke (ask first): detune fader, legato toggle, and
      Assign Mode buttongroup still all reflect the right state
      together; Twin clamp still applies; Legato dim still
      follows mono assign modes.
- [ ] `npm test` and `cargo test -p vxn-ui-web` pass.

## Notes

The reason `makeDetuneLegato` is the only `addCtl` caller is
that it's the only primitive that binds against more than one
id — the controller's normal "one cell, one id" flow doesn't
apply. Surfacing the multi-id contract on the factory's return
value (rather than letting the factory poke the controller's
internals) is what makes the lift safe.

`dispose` is a forward-looking surface. Today the controller's
`rebindAllForLayer` ([dispatch.js:273](../../crates/vxn-ui-web/assets/dispatch.js#L273))
clears `model.controls` and resets `el.innerHTML`, which is the
de facto dispose. Once primitives can mount listeners *outside*
their own `el` (e.g. a future `keydown` global), explicit
`dispose` becomes load-bearing. Setting the contract here is
cheap; reshaping after the fact would be more work.
