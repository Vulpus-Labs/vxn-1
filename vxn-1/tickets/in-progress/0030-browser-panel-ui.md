---
id: "0030"
title: Browser panel UI (folders + presets, search, save form)
priority: medium
created: 2026-05-30
epic: E008
---

## Summary

Replace the popup-over-flat-list preset bar (0027) with a two-pane
**browser panel** in the editor: folders on the left, presets on the
right, a search box up top, a save / new-folder / load form down the
bottom. Sits on the engine IO added by 0029. ADR 0006 §1–§4 owns the
shape.

## Acceptance criteria

- [ ] A toggle-button on the preset bar opens a floating browser panel
  (absolutely positioned, dismissed by clicking the toggle again or by
  loading a preset). Sized large enough to hold ~10 folder rows + ~12
  preset rows.
- [ ] **Folder list (left pane):**
  - [ ] `Factory` header, then each `meta.category` directory of the
    embedded bank (read-only, no edit affordances).
  - [ ] `User` header, then `Uncategorised` first, then each user
    subfolder alpha-sorted.
  - [ ] Clicking a folder selects it and re-renders the preset list.
- [ ] **Preset list (right pane):** the selected folder's presets,
  name-sorted; double-click loads (ADR 0006 §7 — `vxn1-vizia-no-click-slop`,
  use a press-down-with-time-diff implementation rather than vizia's
  `on_double_click` if hover events behave differently in the popup).
- [ ] **Search row (top of panel):** a `Textbox` for free-text + tag tokens,
  with an `x` clear button to its right. Tokens follow `parse_tags`-style
  splitting: `#foo` filters by tag, everything else is a substring match on
  `meta.name`. Multiple `#tag` tokens are AND-ed.
- [ ] **Save form (bottom of panel):**
  - [ ] `Name:` `Textbox` for the preset's display name.
  - [ ] `Tags:` `Textbox` (comma- or whitespace-separated). Empty allowed.
  - [ ] **Save** button — writes to the currently-selected user folder via
    `save_performance_in`. Disabled when a factory folder is selected.
  - [ ] **New Folder** button — calls `create_user_folder("New Folder")`,
    selects the new folder, and enters inline rename on it (the rename
    widget itself ships in 0031; this ticket just wires the "enter rename
    on create" entry point).
  - [ ] **Load** button — loads the selected preset (alongside the double-
    click path).
  - [ ] Status line shows transient messages (Saved, Load failed, warnings,
    etc.) — same idiom as the current preset bar.
- [ ] **Selection populates the save form.** Selecting a user preset fills
  the Name + Tags fields with its current values so an in-place edit + Save
  rewrites the file (this is the path tag editing rides on, ADR 0006 §4).
  Factory preset selection populates the same fields but Name/Tags are
  read-only and Save is disabled.
- [ ] Bulk loads still repaint controls via the existing `PollAutomation`
  idle path — no continuous relayout (`vxn1-vizia-automation-relayout-input-stomp`).
  Verify by interacting with other panels mid-load.
- [ ] The old prev/next steppers + current-preset-name display **stay** on
  the preset bar; they walk the combined Factory+User list in folder-then-
  name order. Their data source uses `list_user_tree` flattened in folder
  order.
- [ ] Tests: a unit test for the search filter shape (substring + AND-of-
  `#tag`), and a unit test for the folder + flatten order used by
  prev/next.

## Notes

- This is the central UI ticket. Inline rename and the context menu live in
  0031 — but this ticket must leave hooks: a `rename_target: SyncSignal<Option<RenameTarget>>`
  the rename view can read, and the New Folder button setting it on the
  created folder.
- Layout: roughly 240 px (folders) + 280 px (presets), plus margins; the
  panel sits below the preset bar with `position_type: Absolute` and a
  high `z_index` so it overlays the faceplate. Sizes are iterative — start
  here and tune in-host.
- The existing `EntrySource` / `BrowserEntry` model can be reused with a
  new `folder: Option<String>` field for user entries (factory entries
  carry `meta.category`).
- Verify in a host with real folders and presets. Don't ship without a
  live test (`vxn1-vizia-no-click-slop`, `vxn1-vizia-automation-relayout-input-stomp`).
  Ask before screen capture (`ask-before-screen-capture`).
