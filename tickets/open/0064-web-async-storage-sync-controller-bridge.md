---
id: "0064"
product: vxn-2
title: "Bridge async browser storage to the synchronous controller loop"
priority: medium
created: 2026-06-15
epic: E019
depends: ["0063"]
---

## Summary

Third ticket of [E019](../../epics/open/E019-web-persistence-presets-state.md).
IndexedDB/OPFS are async; [`PresetStore`](../../crates/vxn-core-app/src/preset.rs#L65)
is synchronous and the controller drains it once per `vxnc_tick`
([web-controller lib.rs:312](../../vxn-1/crates/vxn-web-controller/src/lib.rs#L312)).
Wire the boot-hydration + deferred-write pattern the epic calls for: the store
serves reads from an in-memory cache hydrated before the controller goes live,
and writes fire-and-forget to storage without stalling the tick.

## Design

- **Boot hydration.** The JS glue reads the full user corpus out of storage
  *before* `vxnc_new` / first tick (or seeds the wasm store via an opcode), so
  `list_user_tree` / `user_load` return synchronously from the cache. The
  controller republishes the corpus snapshot after hydration so the faceplate's
  browser populates (the `preset_corpus_changed` path,
  [browser.js](../../vxn-1/crates/vxn-ui-web/assets/browser.js)).
- **Deferred writes.** `user_save` / `user_rename` / `user_delete` /
  `user_move` / folder ops mutate the cache synchronously (so the corpus
  snapshot is correct immediately) and enqueue the persistence op; JS flushes
  the queue to storage off the tick. Must not drop writes on rapid edits or
  page-hide — flush on `visibilitychange`/`pagehide`.
- **Audio path untouched.** No storage call may run on the audio thread; this
  is all main-thread controller work (the audio worklet only reads the param
  SAB).

## Acceptance criteria

- [ ] User presets persist across a page reload (save → reload → still listed
      and loadable).
- [ ] The corpus snapshot the faceplate renders is correct *synchronously*
      after a mutating op (no visible lag waiting on storage).
- [ ] No write is lost under rapid successive saves or a reload immediately
      after a save (flush-on-hide verified).
- [ ] The controller tick does not block on storage I/O (no `await` in the
      sync drain path).

## Notes

- This is the "async vs sync impedance" risk in the epic — the one genuinely
  novel bit of E019. Keep the cache the single source of truth for reads;
  storage is a write-behind mirror.
- Depends on 0063's store + storage choice.
- Out of scope: full-state autosave (0065), export/import (0066).
