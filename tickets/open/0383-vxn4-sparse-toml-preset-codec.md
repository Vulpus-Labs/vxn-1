---
id: "0383"
product: vxn-4
title: "vxn-4 sparse TOML preset codec over vxn-preset"
priority: high
created: 2026-09-10
epic: E052
depends: ["0381"]
---

## Summary

Make a vxn-4 patch representable as a text file: a sparse, name-keyed TOML
document over the descriptor table from
[0381](0381-vxn4-patch-descriptor-table.md), reusing the shared envelope in
[`vxn-preset`](../../crates/vxn-preset/src/lib.rs) (`Meta`, `Header`, `SCHEMA`,
`value_for`, `PresetError`) rather than hand-rolling a third copy of it.

Pure main-thread mapping between a patch and its file text. No IO (that is
[0385](0385-vxn4-user-preset-io.md)), no clap, no UI.

## Acceptance criteria

- [ ] `vxn4-engine::preset` reads and writes a preset file with the shared
      envelope: `schema`, `[meta]`, then vxn-4's body.
- [ ] Only fields deviating from their descriptor default are written, so files
      stay small and auto-adopt improved defaults.
- [ ] Scalars ride a `[params]` table keyed by descriptor name; enum-valued
      fields store their kebab label, not their discriminant.
- [ ] Route **topology** — the offset and scaling sources and curves on each PM
      and bus route, and the matrix slots — serialises as an array of tables,
      keyed by kebab machine names, as vxn-1b splits topology from depth
      ([preset.rs:9-16](../../vxn-1b/crates/vxn1b-engine/src/preset.rs#L9-L16)).
      Only routed entries are written; an absent or unknown name decodes inert.
- [ ] Route **constants stay in `[params]`** and are not duplicated in the
      topology rows — the same depth-is-a-param split vxn-1b makes.
- [ ] Unknown keys and unknown enum labels are **non-fatal warnings** collected
      and returned, never silent and never a hard error. Only a malformed
      envelope or an unsupported schema is `PresetError`.
- [ ] Round-trip property test over randomly generated patches: write, read,
      compare field-by-field.
- [ ] All seven factory patches round-trip, and their rendered audio is
      bit-identical before and after a write/read cycle.

## Notes

Sparse-vs-default is what makes the format survive change, and it has a sharp
edge worth stating in the module doc: a field whose default *changes* silently
changes every preset that did not override it. That is the intended behaviour —
it is how presets adopt a better default — but it means changing a default is a
user-visible act, not an implementation detail.

The `[meta]` shape is fixed by the shared crate and only `name` is required
([lib.rs:28-41](../../crates/vxn-preset/src/lib.rs#L28-L41)). `category` is the
only discriminator the browser groups on; there is no tag list. Do not invent
one here.

Worth deciding in this ticket and recording: whether a preset carries the
**macro assignments** (which routes each of the eight knobs drives, and how far)
as part of the patch. It should — that mapping is the whole reason the eleven
CLAP params are enough
([params.rs:9-16](../../vxn-4/crates/vxn4-clap/src/params.rs#L9-L16)) — but it
means the `Matrix` table is patch state that a preset must carry in full, not
metadata. The mockup's macro row assumes the same thing: labels and ranges are
patch data.

The mockup invents macro labels per patch
([ui-mockup/index.html](../../vxn-4/ui-mockup/index.html), `PATCHES`). If macro
labels are patch data, they belong in this format — probably in `[meta]` or a
`[macros]` table, since they are display strings rather than values. Settle it
here; 0388 will need it.
