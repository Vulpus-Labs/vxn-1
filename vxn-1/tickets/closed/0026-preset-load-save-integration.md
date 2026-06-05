---
id: "0026"
title: Preset load/save integration + host notify
priority: high
created: 2026-05-28
epic: E007
---

## Summary

Wire preset load/save into the live plugin: apply a parsed `Patch` /
`Performance` to the running engine through `SharedParams`, **notify the host** so
its automation lanes and displayed values track the bulk change, read/write the
per-OS user preset directory, and apply the non-automatable `KeyMode` + split
point on a Performance load. This is the one genuinely delicate integration in
E007 — it must keep the single-source-of-truth param model coherent and never
allocate on the audio thread. Decisions:
[ADR 0005](../../adrs/0005-vxn1-presets.md) §5–§6.

## Acceptance criteria

- [x] **Patch load → one layer.** A `LoadPatch { patch, target: Layer }` path
  writes only that layer's params into `SharedParams`; the other layer, the
  global block, key mode and split point are untouched.
  (`SharedParams::load_patch(&PatchValues, Layer)`.)
- [x] **Performance load → everything.** Writes both layers + global into
  `SharedParams`, and applies `KeyMode` + split point on the existing
  non-automatable shared-state path (the same one the `state` blob uses).
  (`SharedParams::load_performance(&PluginState)`.) Key mode is set **plainly,
  not seeded** — a Performance supplies both layers explicitly, so seeding would
  clobber the Lower layer; seed-on-entry stays a discrete-UI-edit behaviour.
- [x] **Host notification.** Chose **emit-on-flush**: a load is a bulk write into
  `SharedParams` (each id gesture-bracketed, the same path as
  `reset_patch_to_defaults`); the existing `LocalParams::fetch_ui_changes` →
  `emit` diff in `vxn-clap` echoes every changed id to the host on the next
  `process`/flush. **Zero `vxn-clap` changes, no audio-thread allocation.**
  Shares the documented deactivated-UI/stopped-transport `request_flush` gap
  (see [[vxn1-status]] DEFERRED) — a preset load while the transport is stopped
  won't echo until processing resumes; not solved here, as the ticket directs.
- [x] **User directory IO.** `vxn-engine::preset_io`: per-OS dir (ADR 0005 §5),
  created on save; `save_patch`/`save_performance` serialize via 0024 and write
  `<name>.toml`; `list_user_presets` enumerates `*.toml`. All main-thread.
- [x] **Warnings surfaced.** `load_preset_file` returns the 0024 warnings
  alongside the parsed preset for the browser (0027) to display; the load never
  fails on a warning.
- [x] Tests: load applied to `SharedParams` yields expected typed values per
  layer; a Patch load leaves global + the other layer byte-identical; a
  Performance round-trips through write→load.

## Notes

- **Threading.** Preset load/save is a main-thread/UI action, not audio. The
  audio thread only ever reads `SharedParams` (and its `LocalParams` mirror) as
  today — do not add IO or serde to any `process` path.
- **Why through `SharedParams`, not straight into the engine:** `SharedParams` is
  the single source of truth (ADR 0001); writing there keeps engine, UI and host
  consistent, and the existing mirror/echo machinery does the rest. A preset load
  is "a lot of edits at once", not a new data path.
- **KeyMode/split are not CLAP params** (ADR 0003 §3/§8) — they cannot be emitted
  as param values; apply them directly and let `state`-style restore handle them.
- Coordinate the host-notify choice with the deactivated-UI `request_flush` gap
  already documented in [[vxn1-status]] (DEFERRED list) — a preset load while the
  transport is stopped is the same edge case; if emit-on-flush is chosen, note
  the limitation, don't try to solve the 'static GUI-handle lifetime here.
- Save-As naming/validation (illegal filename chars, overwrite confirm) can be
  minimal here and polished in 0027.
