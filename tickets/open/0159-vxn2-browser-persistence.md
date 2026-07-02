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

## Acceptance criteria

- [ ] User presets save/load to IndexedDB; the preset browser/preset-bar
      panels list and recall them.
- [ ] Current patch state autosaves and restores across reloads.
- [ ] Patch export/import (file) + URL share-link round-trip a patch
      losslessly through the vxn-2 preset codec.
- [ ] Factory bank (baked `factory.bin`, ticket 0158) loads read-only
      alongside user presets.
- [ ] `std::fs` preset paths in `vxn2-engine` confirmed inert on wasm
      (no panics; browser path uses IndexedDB exclusively).

## Notes

vxn-2 codec: `vxn2-engine/src/preset*.rs` (the `value_for`/`Meta`/`Header`
sparse-TOML shape, see ticket 0143). Reference glue:
`vxn-wasm/web/{preset-persistence,preset-storage,state-autosave,patch-io}.mjs`.
Mirror of vxn-1 E019. Lower priority — instrument plays without it.
