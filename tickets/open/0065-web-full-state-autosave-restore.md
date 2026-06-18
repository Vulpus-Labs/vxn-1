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
