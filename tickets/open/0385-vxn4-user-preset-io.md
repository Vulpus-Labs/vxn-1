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
