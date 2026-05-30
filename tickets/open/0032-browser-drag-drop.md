---
id: "0032"
title: Drag-and-drop preset → folder
priority: low
created: 2026-05-30
epic: E008
---

## Summary

Add drag-and-drop of a user preset onto a user folder to move it. The
target is **best-effort** given known vizia mouse-model fragility
(`vxn1-vizia-no-click-slop`,
`vxn1-vizia-automation-relayout-input-stomp`); the **Move to ▸** submenu
shipped by 0031 is the supported fallback. ADR 0006 §8.

## Acceptance criteria

- [ ] Pressing on a user-preset row and moving the cursor more than ~4 px
  starts a drag. A small "ghost" indicator (the preset name in a styled
  pill) follows the cursor.
- [ ] While dragging, hovering a folder row highlights it as the drop
  target.
- [ ] Releasing over a folder row calls `move_user_preset(path,
  Some(folder))` (or `None` for Uncategorised). Releasing anywhere else
  cancels.
- [ ] Filename collision in the destination surfaces as a status-line
  error; the drag is cancelled and nothing is moved.
- [ ] A drag never originates from a factory row or lands on a factory
  folder (factory is immutable; ADR 0006 §5).
- [ ] Mid-drag, other panels stay interactive (no relayout stomp); a value
  popup on a fader does not steal the drag, and a fader being automated
  does not cancel it.
- [ ] If the drag implementation proves unworkable under vizia in practice,
  the ticket may close as **superseded by Move to ▸** (0031) — the
  fallback is intentional, not graceful degradation.

## Notes

- Implementation outline (ADR 0006 §8):
  - A custom view (`BrowserDragHost`?) sits as the outer container of the
    panel and handles `WindowEvent::MouseMove` / `MouseUp` at the panel
    level — **without** `cx.capture()` on the preset row (capture trapped
    hover events on the folder rows when prototyped).
  - The folder row's `on_hover` writes its `folder_id` into a
    `hover_folder: SyncSignal<Option<FolderId>>`; the preset row's
    `on_mouse_down` writes the drag origin; the host's mouse_move advances
    threshold and ghost position; mouse_up commits the move or clears.
  - A drag indicator is a `Label` styled like the preset row,
    `position_type: Absolute`, bound to the host's cursor signal,
    `hoverable(false)` so it never blocks folder hover.
- Keep all DnD-related state inside the browser panel scope; it must not
  leak into the faceplate or the `PollAutomation` path. If the panel
  rebuilds (e.g. after a folder rename), an in-progress drag cancels —
  that's fine.
- Verify in a host (no screen capture without asking;
  `ask-before-screen-capture`). Real-host testing is the only honest signal
  for whether vizia is cooperating.
