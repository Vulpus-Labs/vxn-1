---
id: "0385"
product: vxn-4
title: "vxn-4 user-preset filesystem IO and the PresetStore adapter"
priority: medium
created: 2026-09-10
epic: E052
depends: ["0383"]
---

## Summary

Put presets on disk. Resolve the per-OS user preset directory, provide the file
operations a browser needs, and implement the `vxn_core_app::PresetStore`
adapter so the shared controller can serve load/save/enumerate without knowing
vxn-4's format.

Ported in shape from
[vxn1b-engine/src/preset_io.rs](../../vxn-1b/crates/vxn1b-engine/src/preset_io.rs),
adapted to vxn-4's codec and its patch shape.

## Acceptance criteria

- [ ] Per-OS user preset directory resolved and created on demand.
- [ ] File ops: load and save a preset, enumerate one level of subfolders,
      create / rename / delete a folder, rename / delete / move a user preset.
- [ ] `vxn_core_app::PresetStore` implemented over those ops, returning
      `PresetLoad` / `PresetMeta` / `UserFolderEntry` / `UserPresetEntry`.
- [ ] The **factory bank needs no IO** — the seven patches stay baked into the
      binary, and the corpus presents factory and user entries as one list, as
      vxn-1b's does.
- [ ] Every mutating call canonicalises its target path and **refuses anything
      outside the user directory**. Test the refusal with `..` traversal and
      with a symlink pointing out of the directory.
- [ ] All filesystem and serde work is main/UI thread. The audio thread touches
      neither, and nothing in this ticket is reachable from `process`.
- [ ] A preset written by this ticket loads back identically, including through
      a folder rename and a move.

## Notes

The path-containment check is the one security-relevant part of E052 and is
worth treating as such rather than as a tidiness rule: a preset name arrives
from user input and ends up as a path component. vxn-1b's `ensure_within_user_dir`
is the shape to copy, canonicalising **after** joining and comparing against the
canonicalised root. Note that canonicalisation requires the path to exist, so the
create-new-file case needs the parent canonicalised instead — that is the usual
place this check is got wrong.

Factory presets are read-only and must stay so. A "save" over a factory patch
writes a user preset with the same name, it does not attempt the bank.

Deliberately out of scope, per E052: categories beyond the single `category`
string the shared `Meta` carries, tagging, search beyond substring, and more than
one level of user folders. vxn-1b settled on one level; matching it keeps the
browser UI in [0389](0389-vxn4-preset-bar-and-browser.md) a port rather than a
design.

## Close-out (2026-09-14)

- [preset_io.rs](../../vxn-4/crates/vxn4-engine/src/preset_io.rs): name
  sanitisation, the user directory, file ops, the byte channel, and
  `EnginePresetStore`. Directory is
  `~/Library/Audio/Presets/Vulpus Labs/VXN4` on macOS,
  `%APPDATA%\Vulpus Labs\VXN4\Presets` on Windows, `$XDG_DATA_HOME/VXN4/presets`
  otherwise — created on demand at the head of every op.
- Full op set, round-tripped on a real tree
  (`preset_operations_round_trip_on_a_real_tree`,
  `folder_operations_round_trip_on_a_real_tree`, `the_listing_walks_one_level_and_sorts`).
  `a_saved_preset_loads_back_identically_through_a_rename_and_a_move` uses patch
  4 (`web`, all 64 routes live) plus labelled macros and compares every field
  bitwise at all three locations.
- **Containment did not port verbatim, and that was correct.** vxn-1b's
  not-yet-exists branch canonicalises only the immediate parent, which is one
  level short for a save into a folder the save itself creates, and assumes the
  base canonicalises to a prefix of the raw parent — false on macOS under
  `/var` → `/private/var`. Replaced with `resolve_for_check`, which peels
  components until one canonicalises and re-attaches the tail resolving `..`
  lexically. `the_escape_guard_resolves_a_tail_that_does_not_exist_yet`.
- Refusals tested against traversal, absolute paths, symlinks out of the tree,
  a not-yet-existing file under such a symlink, and folder deletion through a
  link (`the_escape_guard_refuses_traversal_and_absolute_paths`,
  `the_escape_guard_follows_symlinks_out_of_the_tree`,
  `deleting_a_symlinked_folder_is_refused`,
  `the_preset_operations_refuse_a_path_outside_the_base`). Independently
  re-audited against the public `PresetStore` surface with a separate adversarial
  test before close: every escape refused, the file outside the root intact.
- Factory bank stays baked and read-only
  (`the_factory_bank_loads_through_the_store_without_touching_the_disk`);
  factory and user entries present as one corpus
  (`the_store_presents_factory_and_user_entries_as_one_corpus`);
  `factory_load` bounds-checks rather than letting `patch(i)`'s modulo wrap a
  stale browser index onto a different sound.
- `PresetLoad::blob` is the preset file's own UTF-8 text — no second encoding,
  and it inherits 0383's round-trip guarantees. `encode_blob` / `decode_blob`
  are public so [0386](0386-vxn4-app-crate.md)'s `ParamModel` adopts the same
  convention.
- 16 tests under `preset_io::tests`, all tempdir-based, none touching the real
  preset directory.
- Flagged for its own ticket: `vxn-1b/crates/vxn1b-engine/src/preset_io.rs`
  still has the one-level-parent check. Not exploitable there — its base is
  `$HOME`-rooted and already canonical — but it will refuse a legitimate save
  into a not-yet-created subfolder.
