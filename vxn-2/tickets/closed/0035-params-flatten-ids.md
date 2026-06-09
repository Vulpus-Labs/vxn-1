---
id: "0035"
title: Params — flatten CLAP ID space (drop `upper-` / `lower-`)
priority: high
created: 2026-06-09
epic: E004
---

## Summary

Third ticket of [E004](../../epics/open/E004-single-layer-collapse.md).
Rebuild the CLAP parameter table in `crates/vxn2-engine/src/params.rs`
without per-layer duplication. Every `upper-<id>` / `lower-<id>` ID
collapses to a single `<id>`. `voicing-mode` and `split-point` go away.
Total CLAP-exposed param count: **343 → 179**.

Per [ADR 0002](../../adrs/0002-drop-dual-layer.md).

## Acceptance criteria

- [ ] `op_block_arr!()` and `per_layer_rest_arr!()` macros rewritten as
      a single `op_block_arr!(N)` + `per_patch_rest_arr!()` (or named
      equivalents) that emit unprefixed IDs (`op1-num`, `lfo2-shape`,
      `peg-r1`, `mtx1-depth`).
- [ ] `UPPER` and `LOWER` const arrays in `params.rs` deleted. A single
      `PER_PATCH` array replaces them (the six `op_block_arr!` calls +
      `per_patch_rest_arr!`).
- [ ] `PATCH` array updated: `voicing-mode` and `split-point` entries
      removed. Patch-level count: 19 → 17.
- [ ] `N_LAYERS` constant removed (or set to 1 and the multiplication
      pruned where it survives).
- [ ] `LOWER_BASE` constant removed. `PATCH_BASE` recalculated as
      `N_PER_LAYER` (was `2 * N_PER_LAYER`).
- [ ] `TOTAL_PARAMS` = 162 + 17 = **179**.
- [ ] `VOICING_MODES: &[&str]` constant deleted.
- [ ] `module_for_layer()` helper removed; replace with
      `module_for_section()` (or inline). Module label strings
      simplified: `"Upper / Op 1"` → `"Op 1"`, etc.
- [ ] `module_for_clap_id()` no longer does layer-offset math.
- [ ] Tests:
  - [ ] Delete `upper_lower_have_matching_suffix_ids()` test.
  - [ ] Delete or rewrite the param_sweep test fixtures referencing
        `upper-*` IDs at `crates/vxn2-engine/tests/param_sweep.rs:53,
        158-164` to use the flat IDs.
  - [ ] The "param desc by id" tests at `params.rs:908-987` rewritten:
        `upper-op1-num` → `op1-num`, `upper-algo` → `algo`,
        `upper-mtx1-depth` → `mtx1-depth`, etc. The `lower-*` cases
        deleted (no equivalent IDs exist).
  - [ ] `voicing-mode` desc test (lines ~878-879) deleted.
- [ ] `cargo build -p vxn2-engine` green.
- [ ] `cargo test -p vxn2-engine` green.
- [ ] Manual: dump the full ID list (`cargo test
      -p vxn2-engine list_all_ids --nocapture` if such a test exists,
      else a debug-print test) and confirm 179 entries, all unprefixed.

## Notes

This ticket touches the largest single Rust file in the rip
(`params.rs`, ~1010 lines). Expect ~400 LoC removed.

Sequence: lands after 0033 (`Patch` flat) and 0034 (matrix flat) so the
`mtx_depths` storage on `SharedParams` already carries the flat shape.
This ticket then aligns the CLAP descriptors with that storage.

The `Default` for the (now flat) `Patch` no longer needs a `voicing.mode
= Whole` initialiser — the mode doesn't exist. Default `Patch` is just
the default per-op set + default globals.

If there is any reason to retain a "voicing" parameter for compatibility
with a host preset-recall mechanism, don't — VXN2 is pre-release and the
preset format hasn't shipped. The flat shape is the shipped shape.

Be careful with the module-label strings: the host parameter list
display currently shows `Upper / Op 1 / Ratio`. After this ticket it
shows `Op 1 / Ratio`. The middle `/` separator stays; only the
"Upper " / "Lower " prefix goes. Test by enumerating descriptors and
asserting `module_path()` for a sample of IDs.

CLAP IDs are still stable-by-name (per VXN1 memory
`vxn1-id-stability-dropped` — stability is *not* a binding constraint).
Renames are fine. No `_v2` suffixes, no aliasing, no deprecation
shims.
