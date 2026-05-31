---
id: "0024"
title: Preset format + (de)serialization
priority: high
created: 2026-05-28
epic: E007
---

## Summary

Add a `vxn-engine::preset` module that defines the portable VXN1 preset format
(TOML, keyed by parameter **name**, enums by **label**, sparse) and the pure
conversion to/from the engine value types. No UI, no IO, no clap — just the
format and the mapping, fully unit-tested. This is the foundation the rest of
E007 builds on. Decisions: [ADR 0005](../../adrs/0005-vxn1-presets.md) §1–§3.

## Acceptance criteria

- [x] New module `crates/vxn-engine/src/preset.rs` (re-exported from `lib.rs`).
- [x] serde structs for the §2 schema:
  - top-level `{ schema: u32, kind: "patch"|"performance", meta: Meta }`
  - `Meta { name, author?, category?, tags?: Vec<String>, comment? }`
  - patch body: a map of `PatchParam` **name** → value
  - performance body: `{ key_mode, split_point, global: map, upper: map, lower: map }`
- [x] `Patch` ⟷ `PatchValues` and `Performance` ⟷ `PluginState` conversions:
  - **write is sparse** — omit any param whose value equals its descriptor
    `default` (compare in plain units).
  - **read is default-filling** — start from `PatchValues::default()` /
    `ParamValues::default()`, apply present keys.
  - values clamp on read (reuse `set` / `set_index`, which already clamp).
- [x] **Name lookup**: `PatchParam`/`GlobalParam` by `ParamDesc.name`. Unknown
  key on read → **skip + collect a warning** (return `Vec<String>` of warnings
  alongside the value, don't fail the load).
- [x] **Enum by label**: enum params (de)serialize using the descriptor's
  variant-label array (`WAVE_LABELS` etc.), case-insensitive match on read;
  unmatched label → default + warning. Bools as `true`/`false`. `key_mode`
  serializes as `"Whole"|"Dual"|"Split"` (reuse `KeyMode::label`).
- [x] `to_toml_string(&Patch|&Performance) -> String` and
  `from_toml_str(&str) -> Result<(Patch|Performance, Vec<String> warnings), PresetError>`.
- [x] Tests:
  - round-trip every `PatchParam` and `GlobalParam` (set a non-default value →
    serialize → parse → identical, within f32 exactness for the stored value).
  - sparse: a default `PatchValues` serializes to an **empty** `[patch]` table;
    parsing an empty body yields defaults.
  - unknown key and bad enum label both warn (not error) and fall back to default.
  - a performance round-trips key mode + split point + both layers + global.
  - `schema`/`kind` mismatch is a typed error.

## Notes

- **Why name-keyed, not the binary blob:** CLAP id-stability is dropped
  ([[vxn1-id-stability-dropped]]); the param table reorders freely, so any
  positional/index format rots. `ParamDesc.name` is the stable key. Keep the
  binary `state` blob exactly as-is for the host-session channel — different job
  (ADR 0005 §1).
- Add deps to `vxn-engine`: `serde` (derive), `toml`. Main-thread only; the
  audio path must not gain a serde/toml dependency on any hot call.
- Enum label lookup is the inverse of `ParamDesc::display` for `Enum`; factor a
  shared `variant_index(label) -> Option<usize>` so display and parse can't
  drift. Consider a small alias map per enum if a label ever gets renamed (so old
  presets still parse) — not needed yet, leave a `TODO` hook.
- Keep `meta.category` free-form string; the browser (0027) groups on it.
- This ticket deliberately stops at the format. IO + embedding is 0025; the
  host-sync load path is 0026.
