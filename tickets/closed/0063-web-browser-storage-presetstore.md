---
id: "0063"
product: vxn-2
title: "Choose + implement a browser-storage PresetStore for user presets"
priority: medium
created: 2026-06-15
epic: E019
depends: ["0062"]
---

## Summary

Second ticket of [E019](../../epics/open/E019-web-persistence-presets-state.md).
Replace the `std::fs` user-preset side
([preset_io.rs](../../vxn-1/crates/vxn-engine/src/preset_io.rs)) with a
browser-storage backend behind the existing
[`PresetStore`](../../crates/vxn-core-app/src/preset.rs#L65) trait, so the
faceplate's save / load / rename / delete / move / folder opcodes — already
routed but inert under [`NullStore`](../../vxn-1/crates/vxn-web-controller/src/lib.rs#L170)
— become real. **Decide IndexedDB vs OPFS in this ticket** (see Design).

## Design

- **Storage choice.** Leaning IndexedDB: the existing TODOs already name it
  ([controller.mjs:26](../../vxn-1/crates/vxn-wasm/web/controller.mjs#L26),
  [NullStore doc](../../vxn-1/crates/vxn-web-controller/src/lib.rs#L167)), it is
  universally available, and the corpus is small name-keyed TOML blobs — a
  key→value store fits better than OPFS's file-tree. OPFS is the alternative if
  we want a literal directory mirror. Make the call and record it in an ADR
  addendum / the ticket close-out.
- **Format unchanged.** Reuse the name-keyed TOML
  ([[vxn1-preset-system]]); a "path" becomes a synthetic key (e.g.
  `folder/Name.toml`) so `PresetStore`'s `&Path` surface still works. Keep the
  `sanitize_name` / unique-folder rules from
  [preset_io.rs:72](../../vxn-1/crates/vxn-engine/src/preset_io.rs#L72) so user
  presets behave identically to desktop.
- **Async impedance is 0064.** This ticket builds the storage layer (the JS
  IndexedDB/OPFS module + the wasm-side store that reads/writes against an
  in-memory hydrated cache). The boot-hydration + deferred-write wiring that
  bridges it to the synchronous controller loop is 0064 — keep them separable.
- The store impl lives next to the controller (a `WebPresetStore` in
  `vxn-web-controller`, or a wasm-gated variant in `vxn-engine`).

## Acceptance criteria

- [ ] Storage backend chosen and justified (IndexedDB or OPFS), recorded.
- [ ] A `PresetStore` impl backs user list/load/save/rename/delete/move +
      folder create/rename/delete against browser storage.
- [ ] Folder + filename sanitisation matches the desktop rules (shared code or
      mirrored tests).
- [ ] Saved user presets round-trip: save → list → load reproduces the params.
- [ ] No preset-format change vs desktop (a desktop-saved `.toml` parses).

## Notes

- The trait is synchronous; this ticket may keep writes in the hydrated cache
  and leave actual persistence to 0064's deferred-write path — don't block the
  controller loop on storage I/O here.
- Out of scope: boot hydration timing + deferred-write flush (0064),
  full-state autosave (0065).

## Close-out (2026-06-21)

- **AC1 — backend chosen + recorded.** IndexedDB (not OPFS), justified in the
  [ADR 0009 addendum](../../vxn-1/adrs/0009-web-controller-placement-and-param-store.md).
  Also recorded the **binary-blob** format decision (user-chosen): web user
  presets store the `vxn-app::state` blob + `PresetMeta`, *not* the desktop TOML.
  This **waives AC5** by design — a desktop `.toml` does not parse on web; web
  presets are their own world, cross-platform sharing deferred to 0066. The TOML
  codec (`vxn-engine::preset`) is engine-coupled and not worth hoisting to wasm.
- **AC2 — PresetStore user side.** `WebPresetStore` (now factory + user) delegates
  every user op to an in-memory `UserState`
  ([user_store.rs](../../vxn-1/crates/vxn-web-controller/src/user_store.rs)):
  list/load/save/rename/delete/move + folder create/rename/delete, plus a
  `UserWrite` journal (Put/Delete/PutFolder/DeleteFolder) and `take_journal` /
  `hydrate_*` for 0064. The cache is shared with `ControllerState` via `Arc<Mutex>`.
- **AC3 — sanitisation shared.** `sanitize_name` / `preset_filename` /
  `unique_folder_name` hoisted from `vxn-engine::preset_io` to
  [vxn-app::preset_names](../../vxn-1/crates/vxn-app/src/preset_names.rs); the
  engine now imports them (its duplicate copies + tests removed). Tests live in
  `vxn_app::preset_names::tests` — one source of rules for both backends.
- **AC4 — round-trip.** `vxn_web_controller::user_store::tests::save_list_load_round_trips`
  (+ rename/move/folder/journal+hydration tests, 9 total). The IndexedDB value
  codec round-trips via `vxn-app::preset_record` (`PresetRecord` encode/decode).
- **Storage primitive.** [preset-storage.mjs](../../vxn-1/crates/vxn-wasm/web/preset-storage.mjs)
  (open/getAll/put/delete/applyWrites over two object stores: `presets`,
  `folders`), with a headless fake-IDB smoke test
  ([preset-storage.test.mjs](../../vxn-1/crates/vxn-wasm/web/preset-storage.test.mjs)).
- **Not yet live.** The faceplate's user-preset buttons stay inert in the bridge
  until 0064 wires boot-hydration + deferred-write flush + the controller opcodes
  (the storage *layer* is this ticket's scope; the bridge is 0064).
- Tests green: Rust (vxn-app 18, vxn-web-controller 11, engine 153), node
  (faceplate-bridge / controller / preset-storage), 143 vitest; wasm builds clean.
