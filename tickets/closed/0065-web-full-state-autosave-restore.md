---
id: "0065"
product: vxn-2
title: "Full patch-state autosave + restore on page reload"
priority: medium
created: 2026-06-15
epic: E019
depends: ["0064"]
---

## Summary

Fourth ticket of [E019](../../epics/open/E019-web-persistence-presets-state.md).
On desktop the host persists the plugin-state blob; on the web there is no host,
so the page must autosave the live patch and restore it on reload. The model
already exposes the byte channel:
[`WebModel::snapshot_bytes` / `restore_from_bytes`](../../vxn-1/crates/vxn-web-controller/src/lib.rs#L118)
(plus key-mode/split via `Vxn1Params`). This ticket persists that snapshot and
re-applies it at boot.

## Design

- **Autosave.** Persist the full param snapshot (the same blob the host-state
  analogue uses) to browser storage on change — debounced, and flushed on
  `visibilitychange`/`pagehide`. Reuse 0064's write-behind path; this is one
  more keyed entry, not a new storage mechanism.
- **Restore.** At boot, before the faceplate's `EditorReady` re-broadcast
  ([vxnc_ui_editor_ready](../../vxn-1/crates/vxn-web-controller/src/lib.rs#L496)),
  load the saved snapshot into the model via `restore_from_bytes` so the
  re-broadcast seeds the UI and the param SAB with the restored values. Falls
  back to defaults if no snapshot or it's malformed/wrong-length
  (`restore_from_bytes` already rejects bad lengths).
- **Key-mode/split** are outside the 165 params — persist and restore them too
  (the `Vxn1Params` shared state).

## Acceptance criteria

- [ ] Editing params then reloading the page restores the exact patch (params +
      key-mode + split point).
- [ ] A fresh page with no saved state boots to defaults (no error).
- [ ] A corrupt/old-length snapshot is ignored gracefully (boots to defaults).
- [ ] Autosave does not stall the tick or the audio path (write-behind, like
      0064).

## Notes

- Distinct from user *presets* (0063/0064): this is the single "last session"
  patch, the host-state-blob analogue, not a named entry in the corpus.
- Depends on 0064's storage bridge.
- Out of scope: export/import + share links (0066).

## Close-out (2026-06-21)

- **Snapshot/restore wasm surface.** New C-ABI exports over the existing
  `WebModel::{snapshot_bytes,restore_from_bytes}` (the shared `vxn-app`
  `write_state_bytes`/`read_state_into` codec — params + key mode + split point in
  one canonical blob, byte-identical to native CLAP host state):
  [`vxnc_snapshot_state`](../../vxn-1/crates/vxn-web-controller/src/lib.rs#L1052),
  `vxnc_state_out_ptr`, `vxnc_state_buf_reserve`, and
  [`vxnc_restore_state`](../../vxn-1/crates/vxn-web-controller/src/lib.rs#L1086)
  (returns 0 and leaves the model untouched on a bad/short blob — the codec
  rejects before mutating). JS glue
  [`snapshotState`/`restoreState`](../../vxn-1/crates/vxn-wasm/web/controller.mjs#L371).
- **AC1 (exact patch round-trip).** Restore loads the saved blob into the model
  *before* the `ready`→EditorReady re-broadcast (boot step 2c in
  [faceplate-bridge.mjs](../../vxn-1/crates/vxn-wasm/web/faceplate-bridge.mjs#L407)),
  so the forced full broadcast seeds the UI + param SAB with the restored values;
  key mode + split ride the same blob and republish on the post-tick poll. Proven
  byte-identical (`snapshot → restore → re-snapshot`) + SAB-seeded by
  `state-autosave.test.mjs` and Rust `tests::snapshot_state_round_trips_through_restore`.
- **AC2 (fresh page → defaults).** No saved blob ⇒ `StateAutosave.restore()`
  returns false, model stays at defaults (test: "fresh db restore returns false").
- **AC3 (corrupt/old-length ignored).** Wrong-length and right-length-bad-magic
  blobs rejected, model left at defaults — Rust
  `tests::restore_rejects_bad_blob_without_mutating` + JS AC3 cases.
- **AC4 (no tick/audio stall).** `schedule()` debounces; `flush()` snapshots
  synchronously and chains the IndexedDB `put` on a tail promise off the tick;
  flush-on-`visibilitychange`/`pagehide` backstop. Same write-behind discipline
  as 0064. New module
  [state-autosave.mjs](../../vxn-1/crates/vxn-wasm/web/state-autosave.mjs); storage
  is the same DB with a dedicated `state` store (DB v2, additive upgrade) +
  `getState`/`putState` in
  [preset-storage.mjs](../../vxn-1/crates/vxn-wasm/web/preset-storage.mjs#L102).
  Autosave triggered off patch-state ViewEvents via the bridge's `onPatchChanged`
  hook (excludes pure view-state + corpus-only ops). Bundled by
  [xtask](../../vxn-1/xtask/src/main.rs#L198). All JS suites + `vxn-web-controller`
  Rust tests pass.
