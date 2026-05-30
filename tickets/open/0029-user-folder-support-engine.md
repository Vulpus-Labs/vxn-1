---
id: "0029"
title: User folder support (engine IO)
priority: medium
created: 2026-05-30
epic: E008
---

## Summary

Extend `vxn-engine::preset_io` so the browser can see and mutate one level
of user-side subfolders. Add `list_user_tree`, folder
create/rename/delete, preset rename/delete/move, tag updates, the
`UNCATEGORIZED` label, and `parse_tags` for `#tag`-aware search. Every
mutating call refuses paths outside the user preset directory. The on-disk
file format and the `Performance` <-> `PluginState` mapping are unchanged
(ADR 0006 §1, §5–§6).

## Acceptance criteria

- [ ] `pub const UNCATEGORIZED: &str` — display label for the virtual root
  group (presets directly under `user_preset_dir`). Not a real directory.
- [ ] `UserPreset` carries `tags: Vec<String>` and `folder: Option<String>`
  (`None` = root, `Some(name)` = subfolder).
- [ ] `pub struct UserFolder { name: Option<String>, presets: Vec<UserPreset> }`
  — `None` is the root group, `Some(name)` a real subdirectory.
- [ ] `list_user_tree() -> io::Result<Vec<UserFolder>>` walks one level
  deep: the root group first, then each subfolder alpha-sorted. Empty
  subfolders are kept (a freshly-created folder is empty).
- [ ] `save_performance_in(perf, folder: Option<&str>) -> io::Result<PathBuf>`
  writes into the root or a subfolder (creating the subdirectory if missing).
  `save_performance` becomes the `folder = None` shim.
- [ ] `create_user_folder(suggested) -> io::Result<(PathBuf, String)>` picks
  a unique name: `"New Folder"`, `"New Folder 1"`, … against existing
  folders (case-insensitive); creates the directory; returns path + name.
- [ ] `rename_user_folder(old, new) -> io::Result<(PathBuf, String)>`,
  `delete_user_folder(name) -> io::Result<()>` (recursive),
  `delete_user_preset(path)`, `move_user_preset(path, dest_folder: Option<&str>)`,
  `rename_user_preset(path, new_name)`, `update_preset_tags(path, tags)` —
  all guarded by an `ensure_within_user_dir` helper that canonicalises and
  refuses anything outside the user dir (`PermissionDenied`).
- [ ] `move_user_preset` preserves the on-disk filename (the operation is a
  parent-directory change); `rename_user_preset` updates `meta.name` and
  re-derives the filename via the existing `preset_filename` rules, writing
  the new file and removing the old.
- [ ] `parse_tags(s: &str) -> Vec<String>` — splits on commas + whitespace,
  strips a leading `#` per token, drops empties.
- [ ] Tests: `unique_folder_name` collision suffixing, `parse_tags` shapes,
  the existing round-trip and load-error coverage stays passing.
- [ ] No new dependencies; main-thread only.

## Notes

- The format itself is **unchanged**: this ticket adds nothing to
  `preset.rs`. It only touches `preset_io.rs` and the `vxn-engine` lib re-
  exports.
- Filename / folder-name sanitisation stays as in ADR 0005 (alphanumerics +
  space/`-`/`_`; other chars → `_`; empty → `"Untitled"`). Both share one
  `sanitize_name` helper to avoid drift.
- `ensure_within_user_dir` canonicalises both sides — `fs::canonicalize` may
  fail (the file was just removed); fall back to the lexical path then.
  Belt-and-braces because the UI never *should* hand a bad path, but the
  guard is one line per entry point.
- This is the foundation ticket for E008: 0030 (panel UI) consumes every
  symbol added here.
