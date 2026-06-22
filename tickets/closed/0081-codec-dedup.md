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

## Close-out (2026-06-22)

- `preset.rs`: added a `ParamNamespace` trait (assoc `Values`,
  `from_name`/`desc`/`all`/`get`/`set`) impl'd for `PatchParam`
  over `PatchValues` and `GlobalParam` over `GlobalValues`.
  `apply_patch_table`/`apply_global_table` collapsed to one
  generic [`apply_table::<P>`](../../vxn-1/crates/vxn-engine/src/preset.rs#L213);
  write side `patch_to_table`/`global_to_table` collapsed to
  [`to_table::<P>`](../../vxn-1/crates/vxn-engine/src/preset.rs#L196).
  Call sites are now `to_table::<GlobalParam>` / `::<PatchParam>`
  and `apply_table::<GlobalParam>`; `table_to_patch` delegates.
- `state.rs`: per-patch + global write/read pairs now delegate to
  generic [`write_block`/`read_block`](../../vxn-1/crates/vxn-engine/src/state.rs#L43)
  taking `(count, accessor closure)` — count+accessor copy gone.
  `write_patch`/`read_patch` kept `pub` (module doc references them).
- vxn-app's parallel model-trait `apply_*_table` left untouched
  (out of scope); `app_writer_matches_engine_byte_for_byte`
  parity test still pins the two backends.
- Drift-guard + round-trip tests pass unchanged
  (`codec_matches_legacy_plugin_state`, `default_performance_is_sparse`,
  `bad_enum_label_warns_and_defaults`, `value_clamps_on_read`,
  `unknown_key_warns_and_skips`). `cargo test --workspace` green;
  no new clippy warnings.
