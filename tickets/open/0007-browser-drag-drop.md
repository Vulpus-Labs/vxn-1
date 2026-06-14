---
id: "0007"
product: vxn-1
title: Browser drag-drop — preset row → folder
priority: medium
created: 2026-05-30
epic: E011
---

## Summary

Replace the open ticket 0032 (Vizia drag-drop, never built) with an
HTML5 drag-drop implementation in the 0005 browser. User presets
drag from the right pane onto a user folder in the left pane; the
target folder highlights on hover; drop posts `UiEvent::MovePreset`.

## Acceptance criteria

- [ ] User preset rows are `draggable=true`. Factory rows are not.
- [ ] User folder rows (left pane) are valid drop targets;
      `dragover` highlights them, `dragleave` clears.
- [ ] Drop posts `UiEvent::MovePreset { source: <preset path>,
      target: <FolderKey> }`. Same target as 0006's Move to menu.
- [ ] Dragging onto the current folder is a no-op (target dimmed).
- [ ] Dropping on Factory folders is rejected (no highlight).
- [ ] On corpus refresh after drop, the moved preset stays
      selected and the panel scrolls to it.

## Notes

The earlier vizia ticket 0032 documented the drag-drop requirements;
its acceptance criteria still apply, just the implementation moves
to HTML5 DnD. Once 0007 lands, archive 0032 to closed with a pointer
note that the work shifted to this ticket.

HTML5 DnD has a quirky API. Use `dataTransfer.setData('vxn/preset',
path)` rather than `text/plain` so external dropzones don't
accidentally receive a preset path.

Drag inside a WebView in a CLAP plugin: wry passes pointer events
through; no DAW interference.
