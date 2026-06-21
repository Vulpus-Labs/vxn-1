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

- [x] User presets persist across a page reload (save → reload → still listed
      and loadable). *Headless: `preset-persistence.test.mjs` — a second
      controller hydrated from the same (fake) IDB lists + loads the saves;
      delete persists too. Real-browser IndexedDB confirm is the manual step.*
- [x] The corpus snapshot the faceplate renders is correct *synchronously*
      after a mutating op (no visible lag waiting on storage). *The core
      controller refreshes the shared corpus + the web controller rebuilds
      `corpus_json` in the same `vxnc_tick` drain (on `PresetCorpusChanged`);
      `corpusJson()` reflects the save the same tick — asserted AC2.*
- [x] No write is lost under rapid successive saves or a reload immediately
      after a save (flush-on-hide verified). *Rapid same-name saves collapse
      correctly; `visibilitychange`→hidden / `pagehide` both flush; flush is
      chained on a tail promise so transactions can't race — asserted AC3 +
      flush-on-hide.*
- [x] The controller tick does not block on storage I/O (no `await` in the
      sync drain path). *`takeJournal()` drains synchronously; `applyWrites`
      runs off the tick on the flush tail — asserted AC4.*

## Notes

- This is the "async vs sync impedance" risk in the epic — the one genuinely
  novel bit of E019. Keep the cache the single source of truth for reads;
  storage is a write-behind mirror.
- Depends on 0063's store + storage choice.
- Out of scope: full-state autosave (0065), export/import (0066).

## Implementation (2026-06-21)

**Rust (`vxn-web-controller/src/lib.rs`)**

- User-preset C-ABI opcodes: `vxnc_ui_{save_preset,load_user,rename_preset,
  delete_preset,move_preset,rename_folder,delete_folder,new_folder,step_preset}`.
  Strings ride a reusable arg buffer (`vxnc_arg_buf_reserve`); opcodes take byte
  *lengths* and slice the buffer sequentially. `Option<String>` folder uses
  `len == ARG_NONE` (u32::MAX). Each posts the matching `UiEvent`; the core
  controller does the mutation + journal + corpus refresh on the next tick.
- Corpus republish: `drain_view_events` handles `ViewEvent::PresetCorpusChanged`
  → packs `VE_PRESET_CORPUS_CHANGED` (tag 6, optional follow path) + rebuilds
  `corpus_json` once per drain.
- Deferred flush: `vxnc_take_journal` packs the `UserWrite` journal (`JW_*`
  tags) into an out-buffer; `vxnc_journal_out_ptr` exposes it.
- Boot hydration: `vxnc_hydrate_folder` / `vxnc_hydrate_preset` (decode
  `preset_record`, no journalling) / `vxnc_hydrate_done` (refresh user corpus +
  rebuild JSON). Added `Controller::refresh_user_corpus` pass-through in
  `vxn-app`.

**JS**

- `controller.mjs`: `WebController` user-op methods, `takeJournal()` (decodes to
  `applyWrites` op shapes, copies blob bytes out), `hydrateFolder/Preset/Done`,
  and a `PresetCorpusChanged` decode case.
- `preset-persistence.mjs` (new): `PresetPersistence` — boot `hydrate()`,
  `flush()` (drains sync, applies async on a serialised tail), `attachFlushOnHide`
  (`visibilitychange`/`pagehide`). Storage-unavailable degrades gracefully.
- `faceplate-bridge.mjs`: routes the (formerly inert) user opcodes to the
  controller; `onCorpusChanged` re-pushes `applyPresetCorpus` + flushes; boot
  hydrates before going live.
- `xtask`: bundles `preset-storage.mjs` + `preset-persistence.mjs` into dist.

**Tests** — `cargo test -p vxn-web-controller` (+2 host tests); Node:
`preset-persistence.test.mjs` (new, real wasm + fake-IDB, all 4 ACs),
`faceplate-bridge.test.mjs` (updated: save now journals + fires corpus-changed),
`controller.test.mjs`, `preset-storage.test.mjs` — all pass.
