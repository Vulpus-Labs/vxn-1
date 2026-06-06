---
id: "0017"
title: State save/restore (CLAP state extension)
priority: medium
created: 2026-06-05
epic: E002
---

## Summary

Wire the CLAP `state` extension so the host can serialise the plugin
state into its project file and restore it on reload. The blob is the
parameter table snapshot from 0014's `ParamModel::snapshot_bytes`.

For E002 the snapshot covers only the CLAP param table — the matrix
slots' non-CLAP fields (source / dest / curve, slots 9–16 depth) are
patch-format territory and land with the preset epic. Patches saved by
this ticket carry enough information to reproduce automated knobs but
not custom matrix topologies; for the illustrative default patch
that's fine, the default-patch matrix routes are baked into engine
init code (0018).

## Acceptance criteria

- [ ] `PluginStateImpl` on `VxnMainThread`:
      - `save(output)` reads `ParamModel::snapshot_bytes(&*shared)`,
        writes the whole blob, maps I/O failure to
        `PluginError::Message("state save failed")`.
      - `load(input)` reads `input` to end, calls
        `ParamModel::load_bytes(&mut shared, &blob)`, maps I/O failure
        to `PluginError::Message("state read failed")`.
- [ ] `ParamModel::snapshot_bytes` serialisation format:
      1. 4-byte magic `b"VXN2"`.
      2. 2-byte version `u16 LE` (`= 1`).
      3. 2-byte param count `u16 LE` (`= TOTAL_PARAMS`).
      4. `TOTAL_PARAMS × 4` bytes of `f32 LE` values, indexed by CLAP id.
      No framing per param, no name strings, no descriptor metadata —
      this is the *binary host blob*, not the user-facing preset
      format.
- [ ] `ParamModel::load_bytes` rejects on:
      - Magic mismatch.
      - Version > 1.
      - Length not equal to `8 + count × 4` after the header.
      - `count` not equal to `TOTAL_PARAMS` (version 1 is exact-match;
        future versions will tolerate growth).
      Rejection returns an error variant that maps cleanly to
      `PluginError::Message("state read failed")`.
- [ ] Round-trip test: set every param to a known non-default value
      via `SharedParams::set`, call `snapshot_bytes`, instantiate a
      fresh `SharedParams`, call `load_bytes`, assert
      `SharedParams::get(idx)` matches for every `idx`.
- [ ] Bit-identical: the snapshot blob is byte-for-byte equal to
      itself across two consecutive saves with no intervening param
      changes. Useful for host change-detection.
- [ ] Cross-block consistency: writing a value via host automation
      mid-block, then reading via `save` after `publish`, captures the
      new value (no stale mirror).

## Notes

VXN1's preset format (TOML, name-keyed, per ADR 0005) is *separate*
from the CLAP state blob. Host projects use the blob — fast, compact,
no parser. Users browsing presets use TOML. VXN2 inherits this split:
this ticket only covers the blob. The TOML preset format ships later.

The exact-match `TOTAL_PARAMS` requirement in version 1 means adding
a parameter post-release will require bumping to version 2 with
tolerant loading (fill missing slots from defaults). That's fine for
a v1 release where the table is still settling; revisit when the
first post-1.0 param lands.

Don't store enum *labels* in the blob — store the underlying integer
value. Renaming `lfo1_shape`'s `Tri` variant to `Triangle` should not
invalidate saved projects.

The `state` extension is mandatory for almost every host: Bitwig, FL,
Reaper all require it for session save. Failing `load` silently
breaks projects; the error path here is important. Manual smoke test:
load + save a Bitwig project, modify a param, reload, verify the
modified value persists.
