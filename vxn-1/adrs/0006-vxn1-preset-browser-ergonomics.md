# ADR 0006 — VXN1 preset browser ergonomics (folders, search, tag editing)

- **Status:** Accepted
- **Date:** 2026-05-30
- **Scope:** Browser-side ergonomics on top of the format and load/save path of
  ADR 0005: **user-side folders**, **search (incl. `#tag` filter)**, **tag
  editing**, **rename / delete / move**, **drag-and-drop**. The on-disk format
  is **unchanged**; this ADR is about how the browser presents and mutates the
  user preset corpus.

## Context

E007 / ADR 0005 shipped the preset *format* and a minimal browser (0027):
prev/next over a flat combined Factory+User list, grouped popup, Save-As text
field. It is enough to load a preset; it is not enough to *organise* one.

The gaps users surface immediately:

- **Organisation:** a flat list scales to dozens, not the hundreds of patches a
  working sound bank accumulates. Factory presets already group by category
  (the directory name); user presets have no grouping at all.
- **Findability:** no search, no tag filter — even though `meta.tags` already
  exists in the format (ADR 0005 §2, used by 0028's JP-8 port).
- **Editability:** no rename, no delete, no move. Saving a misspelled name leaves
  a permanent artefact in the user dir; tags can only be set by hand-editing the
  TOML.
- **Mutation safety:** any browser that can delete or rename must refuse paths
  outside the user dir (factory bank is `include_dir!`-baked, but a user dir
  scanned with naive `..` traversal would still be a footgun).

The format is fine as-is — it is name-keyed, sparse, tag-carrying. The work is
all in the **filesystem layout** of the user dir and the **browser surface**.

## Decision

### 1. User folder model: **flat, one level deep**

The user preset directory carries **one level** of subdirectories. No nesting.

- **Top of the user dir** (loose files) is the virtual `"Uncategorised"`
  group — *not* a real subfolder; just a label the browser shows for presets
  living directly under [`user_preset_dir`]. New users land here on first save;
  authors can leave them or move them.
- **Subdirectories** are real folders. Folder = directory; the on-disk name is
  the display name (subject to the same sanitisation as filenames).
- One level matches the factory tree (`<category>/<name>.toml`) and matches how
  hardware-synth and DAW preset browsers actually work. Nesting would invite
  user trees we'd then have to render, search, and drag-target — pay that cost
  only if the flat model proves insufficient in real use.

A folder is created via the browser's **New Folder** button. The suggested name
is `"New Folder"`, auto-suffixed (`"New Folder 1"`, `"New Folder 2"`, …) against
existing folder names. The browser immediately enters **inline rename** on the
new folder so a meaningful name is one keystroke away.

### 2. Save target = currently-selected folder (Uncategorised by default)

Save-As writes into **whichever folder is selected** in the browser. With no
selection (or with Uncategorised selected) the file lands at the top of the user
dir — the legacy behaviour. Selecting a user folder then saving lands the file
there directly, so authors don't curate-then-move.

Saving into a **factory** folder is rejected — factory is immutable (§5). The
Save button greys out while a factory folder is selected.

### 3. Search: substring + `#tag` token filter

A search textbox at the top of the browser filters the visible preset list.
Tokens parse as:

- `#foo` — keep only presets whose `meta.tags` (case-insensitive) contains
  `"foo"`. Multiple `#tag` tokens are AND-ed.
- Anything else — substring match against `meta.name` (case-insensitive). The
  full free-text fragment, not per-token, so `"glass pad"` matches `"Glass
  Pad"` (it doesn't fragment-then-AND).
- An `x` button next to the textbox clears it.

The folder selector itself is **not** filtered; the search only narrows the
**preset list** within the selected folder. A "search across all folders" mode
is *not* in this ADR — once each folder is small the within-folder search is
enough; cross-folder search can come later.

### 4. Tag editing: inline in the save form; context menu for existing files

Tags are edited as a **comma- or whitespace-separated string** in a `Tags:`
textbox next to the save name field. The store is `Vec<String>`; the textbox is
a thin view (`a, b, c` ⟷ `["a","b","c"]`). Leading `#` per token is stripped on
parse (so `"#poly, pad"` and `"poly pad"` both produce `["poly", "pad"]`) —
matching the search syntax means a user can type `#` either way.

- **New preset:** the Save form's `Tags:` field is part of the Save-As payload;
  the resulting TOML carries the tags.
- **Existing user preset:** **Right-click → Edit tags** populates the form
  fields (name + tags) with the preset's current values and selects it; pressing
  Save commits the new tags to the same file. (We do **not** offer a separate
  tag-only modal — fewer code paths, and the user can see-and-edit the name
  too.)
- **Factory preset:** tags display in the save form when a factory preset is
  selected but the field is **read-only** (factory is immutable, §5).

### 5. Mutation safety: factory immutable, every user-side write guarded

- **Factory** presets are read-only by construction (they live inside the
  binary via `include_dir!`, ADR 0005 §4). The browser hides Save / Rename /
  Delete / Move / Edit-tags affordances on factory entries.
- **Every** mutating user-side IO function (`save_*`, `rename_*`, `move_*`,
  `delete_*`, `update_preset_tags`) **canonicalises the target path and refuses
  anything outside the user dir** (`PermissionDenied`). This is non-negotiable
  even though the UI never *should* hand it a bad path — defence in depth, and
  the helper is one line in each entry point.
- Filename and folder name **sanitisation** stays as in ADR 0005 (alphanumerics
  + space/`-`/`_` only; other chars → `_`; empty → `"Untitled"`). Folder names
  share the rule; preset filenames are derived from `meta.name`.
- Conflicts (rename target already exists, move target filename collides) are a
  typed error surfaced in the status line; the browser does **not** silently
  overwrite.

### 6. Rename / move semantics

These are two distinct operations, mapped to two distinct on-disk effects:

- **Rename** changes the **display name** (`meta.name`) and re-derives the
  filename. We **load → mutate `meta.name` → write the new file → remove the
  old**, in that order. (We do not edit the TOML in place to avoid drifting
  serialization formats.) The same `mutate_user_preset` helper carries the tag
  editor — both are "load, mutate, rewrite (possibly under a new name)".
- **Move** changes only the **parent directory**, leaving the on-disk filename
  alone (so a `"My Patch.toml"` filed away under `Pads/` stays `"My Patch.toml"`
  — its `meta.name` is `"My Patch"` either way). Implemented as a plain
  `fs::rename`; refuses to overwrite an existing file in the destination.

### 7. Context menu = Rename, Delete, Move to ▸, Edit tags

Right-click on a row opens a vertical menu (a small popup, dismissed on
outside-click). Available actions depend on the row:

- **User preset:** Rename / Delete / Move to ▸ / Edit tags.
- **User folder:** Rename / Delete (recursive — guarded by an "are you sure"
  status-line confirmation on the first click, committed on the second; we do
  **not** open a modal dialog — the editor is host-windowed and modals are
  fiddly).
- **Factory preset / folder:** menu does **not** open.

The submenu for "Move to ▸" lists user folders (plus `"Uncategorised"` for the
top-of-dir target). Moving to the row's current folder is a no-op.

Inline rename is the same widget rename uses everywhere: the row's `Label` is
replaced by a focused `Textbox`. Commit on Enter; cancel on Escape (or blur). A
sanitised-empty result becomes `"Untitled"` (matches the filename rule), not an
error.

### 8. Drag-and-drop: best-effort, with the context menu as the safety net

Drag a preset row onto a folder row to move it. The implementation uses a
panel-level mouse tracker (so events keep flowing across rows without capture
trapping hover events on the target) and a folder-row hover signal as the drop
target. A drag indicator follows the cursor while active.

Why "best-effort": vizia's mouse model has known fragilities the editor has
already absorbed (`vxn1-vizia-no-click-slop`,
`vxn1-vizia-automation-relayout-input-stomp`). If the drag-drop flow turns out
to drop events in practice, **the Move-to ▸ submenu (§7) is the supported
fallback** — every operation reachable by drag is reachable by menu, so the
user is never blocked.

### 9. Out of scope

- **Multi-select** (selecting and moving/deleting many presets at once). Not
  needed for v1; the menu/drag flow per-row covers the bulk of the workload.
- **Cross-folder search** (search results aggregated across every folder).
- **Tag autocomplete / tag cloud** — the textbox is plain free-text.
- **Favourites** — already deferred in E007, still deferred here.
- **Folder nesting** — see §1.

## Consequences

- New `vxn-engine::preset_io` surface area: `list_user_tree`, `create_user_folder`,
  `rename_user_folder`, `delete_user_folder`, `rename_user_preset`,
  `delete_user_preset`, `move_user_preset`, `update_preset_tags`,
  `save_performance_in`, `parse_tags`, `UserFolder`, `UNCATEGORIZED`. All
  main-thread; the audio path is unchanged.
- `UserPreset` gains `tags: Vec<String>` and `folder: Option<String>` so the
  browser can render and search without re-parsing each file.
- The vxn-ui preset bar is reshaped from a popup over a flat list into a
  two-pane panel (folders | presets) with a search row up top and a save/edit
  row down bottom. The existing preset bar code in `crates/vxn-ui/src/lib.rs`
  (lines roughly 1008-1176 today) is replaced; the editor's `PollAutomation`
  idle path and `SyncSignal` idiom are unchanged.
- A drag-drop wiring is new — and is the riskiest part. The accompanying epic
  ticket separates it (0032) so the rest of the browser ships even if DnD
  proves stuck under vizia's mouse model.
- No on-disk format change. Existing presets keep loading; existing user
  directories (flat) appear under "Uncategorised" with no migration needed.

## References

- ADR 0005 — format + factory + load/save path (this ADR layers on §5–§6).
- Epic E007 — the original preset epic. 0027 (basic browser) ships first; E008
  extends it.
- Memory: `vxn1-vizia-no-click-slop`, `vxn1-vizia-automation-relayout-input-stomp`
  — the vizia mouse-model footguns the browser code must navigate.
- Memory: `vxn1-preset-system` — current preset infra status.
