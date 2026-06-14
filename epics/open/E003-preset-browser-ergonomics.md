---
id: E003
product: vxn-1
title: Preset browser ergonomics (folders, search, tag editing, rename/move/delete)
status: open
created: 2026-05-30
---

## Goal

Turn the basic preset bar shipped by E002 / 0027 into a working **preset
browser**: user-side **folders**, a **search** box with `#tag` filter,
**tag editing**, **rename / delete / move** for user presets and folders,
and **drag-and-drop** between folders. Factory presets stay read-only;
the on-disk format is unchanged.

Decisions recorded in [ADR 0006](../../vxn-1/adrs/0006-vxn1-preset-browser-ergonomics.md).
This epic layers on E002's format + factory + load/save corpus; it does not
touch the format or the audio path.

## Background

E002 shipped the preset *plumbing*: format (0024), embedded factory (0025),
load/save with host echo (0026), and a minimal browser (0027) — prev/next
over a flat combined list, a grouped popup, a Save-As text field.

What's missing once authors actually start collecting patches:

- **Organisation:** user presets all live in one flat directory. Factory
  has categories; user has nothing.
- **Findability:** no search, no tag filter — even though `meta.tags` exists
  in the format and the JP-8 port (0028) was written with tagging in mind.
- **Editability:** no rename, no delete, no move, no tag editing without
  hand-editing the TOML.

ADR 0006 records the chosen shape: flat one-level folders under the user
dir, a virtual `"Uncategorised"` group for top-of-dir files, search with
substring + `#tag` tokens, tag editing inline in the save form, a
right-click menu for rename/delete/move/edit-tags, drag-and-drop as
best-effort with the menu as the fallback.

## Scope

**In:**

- **User folder support (0029, engine):** `list_user_tree`, folder
  create/rename/delete/move, preset rename/delete/move/tag-update, path
  safety guard, `UNCATEGORIZED` constant, `parse_tags` helper. All
  main-thread; no audio-path changes.
- **Browser panel UI (0002):** two-pane folders | presets layout, search
  textbox with `#tag` parsing and a clear `x`, save form with name and tags
  fields targeting the selected folder, **New Folder** button (auto-named +
  inline rename), double-click load, status line.
- **Edit affordances (0003):** inline rename (Enter commit, Escape cancel),
  right-click context menu (Rename / Delete / Move to ▸ / Edit tags),
  factory-side hide of all mutating actions.
- **Drag-and-drop (0032):** drag a preset row onto a folder row to move it;
  panel-level mouse tracker + folder hover signal; drop on Uncategorised
  moves to the user-dir root.

**Out (deferred):**

- **Multi-select** and bulk operations.
- **Cross-folder search** (filter only narrows the selected folder's list).
- **Tag autocomplete / tag cloud.**
- **Favourites** and **preset morphing** (already deferred in E002).
- **Folder nesting** — flat one level only (ADR 0006 §1).
- **CLAP `preset-discovery` integration** (still deferred per ADR 0005 §7;
  the on-disk layout this epic introduces remains discovery-friendly).

## Tickets

- [x] 0029 — User folder support (engine)
- [ ] [0002 — Browser panel UI](../../tickets/open/0002-browser-panel-ui.md)
- [ ] [0003 — Edit affordances: rename, delete, move, tag editor](../../tickets/open/0003-browser-edit-affordances.md)
- [ ] 0032 — Drag-and-drop preset → folder

## Dependency order

```text
0029 (engine IO) ──> 0002 (browser panel)
                         ├──> 0003 (rename / delete / move / tag edit)
                         └──> 0032 (drag-drop)
```

0029 is foundational — it adds the IO surface every UI ticket calls. 0002
ships a working browser (search, save, double-click load) on top of it.
0003 and 0032 are independent of each other on top of 0002; 0003 is the
must-have edit path, 0032 is the nice-to-have layered atop because vizia
mouse-model fragility may bite (ADR 0006 §8) — 0003's Move-to menu is the
fallback if drag stays broken.

## Acceptance

- The browser lists user presets grouped by **folder** (one level deep);
  presets at the top of the user dir appear under `"Uncategorised"`.
- A **search** box filters the selected folder's preset list by substring
  on `meta.name` and by `#tag` tokens against `meta.tags`; `x` clears it.
- **New Folder** creates a uniquely-named subfolder under the user dir and
  drops it into inline rename for editing.
- **Save-As** writes into the currently-selected user folder; the name and
  tags fields populate from the selected preset (for in-place edits) or are
  blank (for new saves). Saving into a factory folder is disabled.
- **Double-click** on a preset row loads it; selecting it and pressing
  **Load** does the same.
- **Right-click** on a user preset offers Rename / Delete / Move to ▸ /
  Edit tags; on a user folder offers Rename / Delete. Factory rows have no
  menu.
- **Drag-and-drop** of a preset onto a folder row moves it (best-effort
  per ADR 0006 §8; the Move-to menu reaches the same operation).
- Every mutating IO call refuses paths outside the user preset directory.
- No audio-thread allocation; no on-disk format change; existing user
  directories appear correctly under `"Uncategorised"` with no migration.
