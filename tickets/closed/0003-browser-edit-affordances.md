---
id: "0003"
product: vxn-1
title: Browser edit affordances (rename, delete, move, tag editor, context menu)
priority: medium
created: 2026-05-30
epic: E003
---

## Summary

Make user presets and folders editable from the browser: **inline rename**,
**right-click context menu** (Rename / Delete / Move to ▸ / Edit tags),
factory-side hiding of all mutating actions. Builds on the engine IO
(0029) and the browser panel (0002). ADR 0006 §4–§7 owns the shape.

## Acceptance criteria

- [ ] **Inline rename:**
  - [ ] The clicked row's `Label` is replaced by a focused `Textbox`. Enter
    commits, Escape (or blur) cancels.
  - [ ] Commit pipes through `rename_user_preset` (for presets) or
    `rename_user_folder` (for folders). Conflicts (`AlreadyExists`) surface
    in the status line; the textbox stays in edit mode so the user can
    correct.
  - [ ] Empty after sanitisation → `"Untitled"` (matches the engine rule),
    not an error.
  - [ ] Entry points: New Folder button (auto-enters rename), right-click
    Rename, and a keyboard shortcut (default F2 if vizia exposes one
    cleanly; if not, the right-click path is enough).
- [ ] **Context menu:**
  - [ ] Right-click on a **user preset** row opens a vertical menu with
    `Rename`, `Delete`, `Move to ▸`, `Edit tags`. Outside-click dismisses.
  - [ ] Right-click on a **user folder** row opens `Rename`, `Delete`.
  - [ ] Right-click on a **factory** row does **not** open the menu.
  - [ ] **Delete** confirms via the status line (first click queues, second
    click commits within ~3 s); no modal dialog (the editor is host-windowed
    and modals are fiddly, ADR 0006 §7).
  - [ ] **Move to ▸** opens a submenu listing user folders + Uncategorised;
    selecting one calls `move_user_preset`. Moving to the current folder is
    a no-op (suppressed). Filename collision in the destination shows the
    error in the status line.
  - [ ] **Edit tags** populates the save form's Name and Tags fields with
    the preset's current values and focuses Tags; the user edits and
    presses Save (which calls `update_preset_tags` if only tags changed, or
    `rename_user_preset` then `update_preset_tags` if the name also
    changed — or just the unified rewrite via the engine's
    `mutate_user_preset` helper).
- [ ] **Factory-side hide:** the Save button, the rename/delete/move/edit-
  tags actions, and the Name + Tags fields' edit state are all read-only
  when the selected entry (or selected folder) is from Factory.
- [ ] Tests: a unit test for the "Move to" target list construction (user
  folders + Uncategorised, current folder suppressed); a unit test for the
  rename → conflict status-line wiring (mock the IO).

## Notes

- Inline rename is the same widget for folders and presets — factor a
  `rename_view(target, signal, commit, cancel)` helper.
- The context menu can be a `Binding`-gated absolutely-positioned `VStack`
  with `ToggleButton`-style rows. Outside-click dismissal: a transparent
  full-panel overlay sat *below* the menu's `z_index` that closes the menu
  on press-down (vizia idiom).
- Delete confirmation via a status-line "press again to confirm" gate
  carries a per-target timestamp and clears after a short window. The
  alternative — a true modal — has bitten editor work before; avoid.
- After any successful mutation, re-run `list_user_tree` and reseed the
  panel's view state (preserve the selection if the entry still exists; on
  rename, follow the renamed entry; on delete, fall back to the folder's
  next entry or none).
- Verify in a host — same as 0002 (no screen capture without asking;
  `vxn1-vizia-no-click-slop`).

## Close-out (won't-do — superseded)

Closed **won't-do 2026-06-19**. Targets the **vizia editor** (inline
`Textbox` rename, `Binding`-gated absolute context menu, vizia outside-click
idiom). Vizia editor retired; HTML faceplate ships instead.

Edit affordances (rename / delete / move / tag edit) are re-homed to the HTML
browser panel → [0005](0005-html-preset-browser-panel.md) (epic E011), over
the same engine IO (0029) and browser-storage persistence
([0063](../open/0063-web-browser-storage-presetstore.md), epic E019).
