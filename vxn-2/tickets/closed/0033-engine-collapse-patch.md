---
id: "0033"
title: Engine — collapse `Patch`, delete `voicing.rs`
priority: high
created: 2026-06-09
epic: E004
---

## Summary

First ticket of [E004](../../epics/open/E004-single-layer-collapse.md).
Rip out the dual-layer infrastructure from the engine kernel: delete the
`voicing` module, flatten `Patch` to a single parameter set, and remove
layer-aware dispatch from the polyphony allocator and engine block
render. Matrix flattening is **0034**; CLAP/param flattening is **0035**;
this ticket lands the kernel shape they depend on.

Per [ADR 0002](../../adrs/0002-drop-dual-layer.md): a patch is one
parameter set. Voicing mode is gone.

## Acceptance criteria

- [ ] `crates/vxn2-engine/src/voicing.rs` deleted. `mod voicing;` removed
      from `lib.rs`. All `use crate::voicing::*` imports gone from
      `engine.rs`, `alloc.rs`, `matrix.rs`, `params.rs`.
- [ ] `Patch` struct (currently in `voicing.rs`) collapses to a single
      parameter set. Move what was `LayerParams { stack, voice }` into
      `Patch` directly (so `Patch { stack, voice, matrix }`), or keep
      `LayerParams` renamed to `PatchParams` if downstream call sites
      read more clearly with the indirection — pick whichever leaves the
      flatter call graph and document the choice in the PR description.
- [ ] `Patch::upper`, `Patch::lower`, `Patch::split_layer`, `Patch::layer`,
      and the `voicing: VoicingParams` field are removed.
- [ ] `PolyAlloc::layers: [Layer; N_STACKS]` field removed.
      `PolyAlloc::stack_layer()` removed. Allocator no longer carries a
      per-stack layer tag.
- [ ] `PolyAlloc::note_on_patch()` becomes a single-path allocation:
      one stack per note, no `VoicingMode` match, no Layer/Split branch.
- [ ] `Engine::process_block` no longer demuxes matrix-table choice by
      `alloc.stack_layer(i)`; it reads the single `MatrixTable` directly.
      (This depends on **0034** for the matrix-side change — land 0033
      first behind a transient `matrix.upper` reference, then 0034
      collapses the field. Or land both together as a sequenced pair.
      Document the chosen sequencing in the PR.)
- [ ] `Engine::snapshot_from_shared` (or equivalent) no longer
      propagates `voicing-mode` or `split-point` — those params no
      longer exist. The snapshot path operates on the flat parameter
      set.
- [ ] All voicing-related tests in `alloc.rs` deleted:
      `patch_layer()`, `patch_split()`, `test_layer_mode_*`,
      `test_split_*`. The single-layer tests stay.
- [ ] The `snapshot_propagates_voicing_change` test in `engine.rs`
      deleted.
- [ ] `cargo build -p vxn2-engine` green.
- [ ] `cargo test -p vxn2-engine` green (single-layer tests pass; no
      layer fixtures left).
- [ ] Default patch (`default_patch.rs`) no longer references
      `upper`/`lower`/`voicing` — it constructs the flat shape directly.

## Notes

The order between 0033 and 0034 (matrix) is the only real design choice.
Two options:

1. **Sequenced** — land 0033 with the matrix temporarily flattened by
   keeping only `matrix.upper` and renaming references inline.
   0034 then renames the field to `matrix` and deletes the
   `Layer`/`PatchMatrix` types. Two PRs, each green.
2. **Joint** — land 0033 + 0034 in one PR. Smaller total diff, but a
   bigger single review.

Default to option 1 unless the joint diff is genuinely small. Per
preference for splitting into reviewable chunks.

`LayerParams` is currently `{ stack: StackParams, voice: VoiceParams }`.
After this ticket, either flatten those two into `Patch` directly or
rename `LayerParams → PatchParams`. The PR should pick the one that
reads better in `Engine::process_block` after the change. The matrix
slots table currently lives next to (not inside) `Patch`; it stays where
it is and gets flattened in 0034.

The `default_patch.rs` currently sets `voicing_mode = Layer` as the
default — verify this is the *only* place `Layer` is the default;
adjusting it to `Whole` while the field still exists is a no-op for the
deletion path but worth confirming during the rip.

The integration tests in `crates/vxn2-engine/tests/param_sweep.rs`
include `upper-algo` / `lower-algo` parameter assignments — those are
0035's territory, but if `cargo test --workspace` is run during this
ticket, those tests will fail to compile against the new flat `Patch`.
Either gate this ticket on landing alongside 0035 or stub the test out
temporarily; the PR plan must say which.
