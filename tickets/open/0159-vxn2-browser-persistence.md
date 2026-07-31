---
id: "0159"
product: vxn-2
title: vxn-2 browser persistence — IndexedDB presets, autosave, patch-io
priority: low
created: 2026-06-30
epic: E030
---

## Summary

Browser-side persistence for vxn-2: user presets in IndexedDB, current-
state autosave/restore, and patch export/import + URL share-link. vxn-2's
preset format is TOML+serde+`include_dir` (vxn-1 used a binary blob), so
this wraps the existing `vxn2-engine` codec in an IndexedDB adapter rather
than copying vxn-1's blob path verbatim. Ports the
`preset-persistence.mjs` / `preset-storage.mjs` / `state-autosave.mjs` /
`patch-io.mjs` patterns.

## Status: COMPLETE — full persistence shipped 2026-07-26

Factory-load shipped 2026-07-12; the remaining user presets + autosave + patch-io
scope shipped 2026-07-26 (close-out below).

## Acceptance criteria

- [x] User presets save/load to IndexedDB; the preset browser panel lists and
      recalls them. The web controller gained a `WebPresetStore` (in-memory
      `UserState` cache + write journal, `user_store.rs`) replacing the
      factory-only stub, plus `vxnc_ui_save_preset` / `_load_user` /
      `_rename_preset` / `_delete_preset` / `_move_preset` / `_new_folder` /
      `_rename_folder` / `_delete_folder`, an arg-staging buffer, a
      `vxnc_take_journal` drain, and `vxnc_hydrate_folder` / `_preset` / `_done`
      boot hydration. `preset-storage.mjs` (IndexedDB `vxn2-presets` DB) +
      `preset-persistence.mjs` (hydrate-before-live + deferred write-behind flush)
      wire it; the bridge un-defers the panel opcodes and republishes the corpus +
      flushes the journal on `preset_corpus_changed`. Proven end-to-end against
      the real wasm (`preset-persistence.test.mjs`): save → flush → a fresh
      controller hydrated from the same db lists + loads them.
- [x] Current patch state autosaves and restores across reloads. `state-autosave.mjs`
      debounces `snapshotState()` writes to a dedicated `state` store and restores
      via `restoreState()` at boot before the re-broadcast; `vxnc_snapshot_state` /
      `_restore_state` back it. `state-autosave.test.mjs` proves a byte-identical
      patch round-trip across a reload + graceful corrupt-blob handling.
- [x] Patch export/import (file) + URL share-link round-trip a patch losslessly
      through the vxn-2 preset codec. `patch-io.mjs` (file `.toml` export/import
      via `exportToml` / `importToml`, `#patch=` base64url share-link) reuses
      `vxn2_engine::preset::{write_preset,from_toml_str}`; `applyShareLinkOnBoot`
      restores a share-link at boot. `patch-io.test.mjs` proves export→import
      re-export byte-identity + a share-link byte-for-byte round-trip on real wasm.
      (No faceplate button yet — exposed on `window.__vxn.{exportPatch,importPatch,
      shareLink}` for a future UI.)
- [x] Factory bank (baked `factory.bin`, ticket 0158) loads read-only: the web
      controller gained a `WebFactoryStore` + factory C-ABI
      (`vxnc_factory_buf_reserve` / `vxnc_load_factory` / `vxnc_corpus_json_*` /
      `vxnc_ui_load_factory` / `vxnc_ui_step_preset`); the bridge fetches
      `factory.bin` on boot, parses it, and hands the corpus to the preset
      browser; `load_factory` / `step_preset` opcodes route to the controller;
      `PresetLoaded` view events decode to the faceplate. Proven end-to-end
      against the real wasm + baked 204-preset bank.
- [x] `std::fs` preset paths in `vxn2-engine` inert on wasm: the web controller
      never calls `Vxn2PresetStore` (whose `user_*` methods are the only `std::fs`
      users) — it constructs its own `WebPresetStore` over the in-memory
      `UserState` + IndexedDB. The factory bank is `include_dir!`-embedded and
      user presets are stored as TOML in IndexedDB, so no wasm code path touches
      `std::fs`; the controller builds + runs clean.

## Close-out (factory-load, partial, 2026-07-12)

Factory-load done. Rust: `vxn2-web-controller` `WebFactoryStore` +
`parse_factory_bin` + 6 factory opcodes + `VE_PRESET_LOADED` packing
(`corpus_snapshot_json` re-exported from `vxn2-app`). JS: `controller.mjs`
`loadFactoryAsset` / `corpusJson` / `loadFactory` / `stepPreset` +
`preset_loaded` decode; `faceplate-bridge` fetches the bank on boot + routes the
preset opcodes (user ops in `DEFERRED_OPS`). Tests: controller Rust
`factory_bin_round_trips_and_loads`; node `controller-wasm.test.mjs` (real wasm +
`factory.bin`) + decode/routing cases — full web suite **50** green.

## Close-out (user presets + autosave + patch-io, 2026-07-26)

Full persistence shipped. **Reused, not rebuilt:** `SharedParams` snapshot/load
bytes (state blob), `preset::{write_preset,from_toml_str}` (TOML codec — no fs),
the `Controller` user-op handlers + `PresetCorpusChanged`, `corpus_snapshot_json`.

**Rust** (`vxn2-web-controller`): new `user_store.rs` — `UserState` (BTreeMap
cache + `UserWrite` journal) storing each preset as its **TOML** record (vxn-2
desktop stores TOML too, so web ↔ desktop can't drift); `WebFactoryStore` →
`WebPresetStore` (factory bank + `Arc<Mutex<UserState>>`, full `PresetStore`
impl). New C-ABI: `vxnc_arg_buf_reserve`, `vxnc_ui_{save_preset,load_user,
rename_preset,delete_preset,move_preset,new_folder,rename_folder,delete_folder}`,
`vxnc_take_journal` / `_journal_out_ptr`, `vxnc_hydrate_{folder,preset,done}`,
`vxnc_snapshot_state` / `_state_out_ptr` / `_state_buf_reserve` / `_restore_state`,
`vxnc_export_toml` / `_toml_out_ptr` / `_toml_buf_reserve` / `_import_toml`. New
wire records `VE_CORPUS_CHANGED` (7) + `VE_STATUS` (8), `PRESET_SRC_USER`, journal
`JW_*` tags. `vxn2-engine`: `sanitize_name` / `preset_filename` /
`unique_folder_name` made `pub` (shared with the web store, no drift).

**JS** (`vxn2-wasm/web`): ported `preset-storage.mjs` (`vxn2-presets` IndexedDB,
presets/folders/state stores), `preset-persistence.mjs`, `state-autosave.mjs`,
`patch-io.mjs`. `controller.mjs`: arg-staging + `savePreset` / `loadUser` / rename
/ delete / move / folder ops, `hydrate*`, `takeJournal`, `snapshotState` /
`restoreState`, `exportToml` / `importToml` (patch-swap pulse on restore/import),
`VE_CORPUS_CHANGED` / `VE_STATUS` / user-`PresetLoaded` decode. `faceplate-bridge`:
un-defers the user opcodes + routes them, `_initPersistence` (hydrate → republish
→ share-link-or-autosave restore → flush-on-hide, all before the queued `ready`),
`_onEvents` (corpus-changed → republish + flush; patch-state change → autosave
schedule), `window.__vxn.{exportPatch,importPatch,shareLink}`. `xtask` bundles the
4 new modules.

**Tests:** Rust `vxn2-web-controller` **21** (user store round-trips, journal wire,
snapshot/restore, export/import TOML, save→corpus-changed+journal,
hydrate→load-user), `vxn2-engine` **216** — all green. Node web suite **89** green
(4 new files: `preset-storage`, `preset-persistence`, `state-autosave`, `patch-io`
— the last three over the real wasm from `web-dist`). Manual DAW/browser verify
(save/reload/share in a browser) still pending on the user.

## Notes

vxn-2 codec: `vxn2-engine/src/preset*.rs` (the `value_for`/`Meta`/`Header`
sparse-TOML shape, see ticket 0143). Reference glue:
`vxn-wasm/web/{preset-persistence,preset-storage,state-autosave,patch-io}.mjs`.
Mirror of vxn-1 E019. Lower priority — instrument plays without it.
