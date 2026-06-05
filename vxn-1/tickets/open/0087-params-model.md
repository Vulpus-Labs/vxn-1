---
id: "0087"
title: Wrap params descriptor as a model object
priority: medium
created: 2026-06-01
epic: E017
---

## Summary

`window.vxn.params` is currently a bare object — callers walk it
with raw `for ... in` loops
([dispatch.js:7](../../crates/vxn-ui-web/assets/dispatch.js#L7))
and indexed `[id]` reads
([dispatch.js:31](../../crates/vxn-ui-web/assets/dispatch.js#L31)).
Wrap it as a small `params` model object with `.get(id)`,
`.idByName(name)`, `.idByNameAtLayer(name, layer)`, `.variantIdx(paramName,
variantName, layer)`, and `.patchCount`. The wrapper holds the
cached reverse index from 0084 internally.

Decouples primitive / controller code from the global; makes the
params surface mockable in tests and replaceable by vxn-2's own
descriptor table.

## Acceptance criteria

- [ ] [bridge.js](../../crates/vxn-ui-web/assets/bridge.js) or
      a new `params.js` factory exposes `createParamsModel(raw,
      patchCount)`:
      ```js
      // raw: the splice-time JSON object keyed by string id.
      // patchCount: the per-patch slot count for layer translation.
      function createParamsModel(raw, patchCount) {
        const byName = new Map();
        for (const k in raw) {
          const id = parseInt(k, 10);
          const desc = raw[k];
          // Lowest-id wins for per-patch params: Upper-layer id.
          if (!byName.has(desc.name) || id < byName.get(desc.name)) {
            byName.set(desc.name, id);
          }
        }
        return {
          patchCount,
          get(id)                    { return raw[id]; },
          idByName(name)             { return byName.get(name) ?? null; },
          idByNameAtLayer(name, lyr) { /* … +patchCount for per-patch */ },
          variantIdx(p, v, lyr)      { /* … */ },
        };
      }
      ```
- [ ] `window.vxn.params` continues to exist as the bare object
      (bridge.js sets it at boot time) — `createParamsModel` reads
      from it but the controller passes the *wrapper* everywhere
      from `init()` onward.
- [ ] [dispatch.js](../../crates/vxn-ui-web/assets/dispatch.js)
      `paramIdByName`, `paramIdByNameAtLayer`, `variantIdx` either
      delegate to the params model (intermediate state) or are
      deleted entirely once 0088 wires the model into the
      controller.
- [ ] If 0084 has landed: its `_paramIdByName` cache moves into
      the params model's closure; the module-level cache is
      deleted. Close 0084 as subsumed if not already done.
- [ ] [crates/vxn-ui-web/assets/__tests__/params-model.test.js](../../crates/vxn-ui-web/assets/__tests__/params-model.test.js)
      covers (using the 0080 fixture):
      - `get(id)` returns the same descriptor object as `raw[id]`.
      - `idByName(name)` returns the lowest id for per-patch
        params and the unique id for globals.
      - `idByName` returns `null` for unknown names.
      - `idByNameAtLayer('foo', 'lower')` translates per-patch
        ids by `+patchCount`; leaves globals unchanged.
      - `variantIdx('filter_mode', 'Notch', 'upper')` returns the
        correct enum index; `'unknown_variant'` returns -1.
- [ ] Manual smoke (ask first): every panel still binds correctly,
      every dim rule still resolves on layer flip.
- [ ] `npm test` and `cargo test -p vxn-ui-web` pass.

## Notes

The model wraps but doesn't shadow `raw` — it holds a reference
and reads through. If `window.vxn.params` is reassigned (it isn't
today; the contract holds), the model becomes stale and would
need rebuilding. Acceptable since the contract is "splice-time
constant".

`variantIdx` is on the model rather than free-floating because
its only useful inputs are paramName + variantName + layer, which
are exactly what the model knows how to resolve. Keeping it as a
method makes the test setup smaller and the call sites read like
"ask the model for the index" rather than "look up via a free
function that happens to read globals".

The `??` (nullish coalescing) operator is supported by
WKWebView's modern JavaScriptCore — no transpile needed. Confirm
the same on the Vitest side (Node 20+ has it).
