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

## Status: PARTIAL — factory-load subset shipped 2026-07-12 (rest deferred)

Scoped down to **factory presets only** for this pass; user-preset persistence
(IndexedDB save/load), state autosave, and patch export/import + share-link are
deferred. Ticket stays **open** for that remaining work.

## Acceptance criteria

- [ ] User presets save/load to IndexedDB; the preset browser/preset-bar
      panels list and recall them. — **DEFERRED**
- [ ] Current patch state autosaves and restores across reloads. — **DEFERRED**
- [ ] Patch export/import (file) + URL share-link round-trip a patch
      losslessly through the vxn-2 preset codec. — **DEFERRED**
- [x] Factory bank (baked `factory.bin`, ticket 0158) loads read-only: the web
      controller gained a `WebFactoryStore` + factory C-ABI
      (`vxnc_factory_buf_reserve` / `vxnc_load_factory` / `vxnc_corpus_json_*` /
      `vxnc_ui_load_factory` / `vxnc_ui_step_preset`); the bridge fetches
      `factory.bin` on boot, parses it, and hands the corpus to the preset
      browser; `load_factory` / `step_preset` opcodes route to the controller;
      `PresetLoaded` view events decode to the faceplate. Proven end-to-end
      against the real wasm + baked 204-preset bank.
- [~] `std::fs` preset paths in `vxn2-engine` inert on wasm: the factory path
      never touches `std::fs` (the bank is `include_dir!`-embedded and the blobs
      restore in memory); the wasm controller builds + runs clean. Full audit of
      the `user_preset_dir` paths rides the deferred user-persistence work.

## Close-out (partial, 2026-07-12)

Factory-load done. Rust: `vxn2-web-controller` `WebFactoryStore` +
`parse_factory_bin` + 6 factory opcodes + `VE_PRESET_LOADED` packing
(`corpus_snapshot_json` re-exported from `vxn2-app`). JS: `controller.mjs`
`loadFactoryAsset` / `corpusJson` / `loadFactory` / `stepPreset` +
`preset_loaded` decode; `faceplate-bridge` fetches the bank on boot + routes the
preset opcodes (user ops in `DEFERRED_OPS`). Tests: controller Rust
`factory_bin_round_trips_and_loads`; node `controller-wasm.test.mjs` (real wasm +
`factory.bin`) + decode/routing cases — full web suite **50** green.

**Remaining (keep open):** IndexedDB user presets (`preset-storage` /
`preset-persistence`), state autosave (`state-autosave`), patch export/import +
`#patch=` share-link (`patch-io`), and the matching controller opcodes
(`vxnc_ui_save_preset` / `_load_user` / `_snapshot_state` / `_restore_state` /
`_export_toml` / `_import_toml` + the user_store). Reference:
`vxn-wasm/web/{preset-storage,preset-persistence,state-autosave,patch-io}.mjs`.

## Notes

vxn-2 codec: `vxn2-engine/src/preset*.rs` (the `value_for`/`Meta`/`Header`
sparse-TOML shape, see ticket 0143). Reference glue:
`vxn-wasm/web/{preset-persistence,preset-storage,state-autosave,patch-io}.mjs`.
Mirror of vxn-1 E019. Lower priority — instrument plays without it.
