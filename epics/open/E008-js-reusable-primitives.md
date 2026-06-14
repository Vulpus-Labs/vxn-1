---
id: E008
product: vxn-1
title: Faceplate JS reusable primitive library — vxn-2 lift prep
status: open
created: 2026-06-01
---

## Goal

Decouple the faceplate JS primitives from `window.vxn` globals so
they can be lifted into vxn-2 verbatim. VXN-2 will share sliders,
selection buttons, rotary dials, and the same MVC dispatcher shape;
the work here is to factor the current monolith into View
primitives, a Controller factory, and a Model (param table +
last-known cache) that all communicate via injected dependencies
rather than globals.

This is the long-tail epic on the faceplate JS. It depends on the
test net from E015 and lands cleanest
after the E016 boundary cleanup
(generalised drag scaffolding + paintFader / tgRow helpers all live
in the lifted primitive library), but the two can be sequenced in
either order if dependencies are honoured per-ticket.

## Background

The 0075 close-out audit (E007) noted that the primitive factories
are ~80% of the way to a clean MVC factoring already: each `makeX`
takes `(el, id, desc, opts?)` and returns `{ update }`, and the
dispatch / model split lives in [dispatch.js](../../crates/vxn-ui-web/assets/dispatch.js).
The remaining 20% is:

- **N1** Primitives reach into `window.vxn.send` directly (every
  click/drag handler in `panels.js`). Should be an injected
  dependency.
- The param descriptor table is a `window.vxn.params` bare object;
  callers walk it with raw `for ... in` loops or `[id]` lookups.
  Wrap as `params.get(id) / idByName(name) / idByNameAtLayer(name,
  layer)` with a cached reverse index (overlaps with E016's 0084 —
  if 0084 lands first, 0087 promotes the cached function to a
  proper params object).
- The dispatcher (`model`, `bindCell`, `rebindAllForLayer`,
  `applyDimRulesFor`) is module-level singletons in `dispatch.js`.
  Promote to `createController({ params, send, primitives })` — a
  factory that returns the dispatch object. Each primitive is
  registered against its `data-control` kind, so `bindCell`
  dispatches on a lookup table rather than a `switch`.
- `makeDetuneLegato` calls `addCtl` from inside the factory — the
  only primitive that does. Lift those three calls back into
  `bindCell` (the controller) so every factory has the same
  zero-side-effect contract: `(el, id, desc, opts) → { update,
  dispose }`.
- Source files reorganise into folders: `primitives/`,
  `controller/`, `browser/`, `bridge/`. The splice loader walks
  these directories and concatenates in dependency order. The
  ESM source from E015 makes this safe.

The reuse target is vxn-2 importing the `primitives/` and
`browser/` directories verbatim, with a vxn-2-specific
`bridge/` and `controller/init.js` that wires its own param table
and ipc bridge.

## In scope

- Inject `send` (and any other host capability) into every
  primitive factory.
- Wrap `window.vxn.params` as a small `params` model object.
- Promote the dispatcher to a `createController({ params, send,
  primitives })` factory; primitives registered by kind.
- Unify primitive return shape (`{ update, dispose }`); remove
  `addCtl` from `makeDetuneLegato`.
- Reorganise `assets/` into folders; update the splice loader and
  the substring suite.

## Out of scope

- The actual vxn-2 lift (separate epic when vxn-2 starts).
- Building a published npm package — the lift target is a
  source-level copy, not a registry artefact.
- TypeScript / a build step beyond the existing splice loader.
- HTML / CSS reorganisation.
- New controls, new panels, or any behavioural change.

## Phasing

E015 (test net) is a hard prerequisite — every ticket lands a
behavioural assertion. E016 is recommended-but-not-strict; if E016
hasn't landed, this epic still works but produces slightly more
shoes-into-house refactor noise.

1. **0086** Inject `send` into every primitive factory (N1).
   No `window.vxn.send` reads inside `panels.js`. The controller
   passes `send` through `bindCell` to each factory's opts.
2. **0087** Wrap params as a model object. If E016/0084 has
   landed, promote the cached reverse-index function to a `params`
   object; if not, build both at once.
3. **0088** Controller-as-factory: `createController({ params,
   send, primitives })`. `model` becomes the factory's local
   state; `init()` constructs and returns it. Primitives
   registered by `data-control` kind.
4. **0089** Unify primitive contract — `addCtl` moves out of
   `makeDetuneLegato` into the controller's `bindCell` for the
   composite case (three registrations driven from the composite's
   declared id set, not from inside the factory).
5. **0090** Source-tree reorganisation into `primitives/`,
   `controller/`, `browser/`, `bridge/` folders. Splice loader
   walks each directory in dependency order. Substring tests
   update to match the new paths.
6. **0091** Modal mount-target audit (N7 from 0075). If no second
   use case has emerged, leave a documented note in
   `browser/modal.js` flagging the lift-out trigger. If a second
   use case has emerged (likely from another panel needing
   confirms), lift `mountModal` into a standalone `controller/modal.js`
   helper.

0086 → 0088 has a natural ordering (you can't make the controller a
factory while primitives still read `window.vxn.send` from the global
namespace). 0087 can run in parallel with 0086. 0089 follows 0088.
0090 lands last among the structural tickets. 0091 is a
forward-looking audit-only ticket.

## Tickets

- [ ] 0086 — Inject `send` into primitive factories
- [ ] 0087 — Wrap params descriptor as a model object
- [ ] 0088 — Controller-as-factory + primitive registry
- [ ] 0089 — Unify primitive return shape; lift addCtl out of composite
- [ ] 0090 — Source-tree reorganisation into primitives/ controller/ browser/ bridge/
- [ ] 0091 — Modal mount-target audit

## Acceptance

- All six tickets closed.
- `grep window.vxn.send` returns hits only inside `bridge/` (the
  module that defines it) and inside `controller/init.js` (the
  one bootstrap site that captures it before injecting into the
  controller).
- `grep window.vxn.params` returns hits only inside `bridge/` and
  inside `controller/params.js` (the wrapper).
- Every primitive factory has the signature `(el, id, desc, {
  send, ...opts }) → { update, dispose }`. No factory calls
  `addCtl` directly.
- `createController` is the single entry point; `dispatch.js`'s
  current module-level globals are replaced by the controller's
  closure-local state.
- The source tree under `assets/` has the four-folder layout.
- `npm test` and `cargo test -p vxn-ui-web` both pass.
- Manual smoke confirms zero regression (ask first per
  `ask-before-screen-capture`).
- A forward note in `assets/README.md` summarises the lift
  contract for vxn-2: "import primitives/, browser/; write your
  own bridge/ and controller/init.js."
