---
id: "0081"
product: vxn-1
title: vxn-engine — collapse duplicated param-table and state codecs
priority: medium
created: 2026-06-21
epic: E024
---

## Summary

Byte-identical codec paths that differ only by param
namespace.

`apply_patch_table` and `apply_global_table`
(`vxn-engine/src/preset.rs:211-240`) are identical except
`PatchParam::from_name`/`p.desc()` vs `GlobalParam::from_
name`/`g.desc()` — same "for each key: resolve name →
param → parse → set, warn on miss" loop. The same shape
repeats in `state.rs`'s `write_patch`/`read_patch` vs
`write_global`/`read_global`, which differ only by a count
and an accessor.

Two copies means a change to the warning text or parse
policy has to land twice. Low blast radius (short, stable
bodies) but pure redundancy.

## Acceptance criteria

- [ ] One generic `apply_table` (parameterized by a small
      `ParamNamespace` trait carrying `from_name`/`desc`/
      `set`, or by a `macro_rules!`) serves both patch and
      global preset application; `apply_patch_table` /
      `apply_global_table` become one-line call sites or are
      removed.
- [ ] The `state.rs` write/read pairs collapse onto a
      generic `write_block(count, |i| get(i))` /
      `read_block(count, |i, v| set(i, v))` (or trait
      equivalent), eliminating the count+accessor copy.
- [ ] The existing preset/state round-trip and drift-guard
      tests (`codec_matches_legacy_plugin_state`, sparse
      defaults, bad-enum clamp) still pass unchanged —
      they pin that the shared path is behaviour-identical.
- [ ] `cargo test --workspace` green.

## Notes

Both synths are heading toward this pattern — if the trait
falls out cleanly, consider whether it belongs in
`vxn-core-app` for vxn-2 to share, but that is optional and
should not block the vxn-1 dedup.
