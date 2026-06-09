---
id: "0034"
title: Matrix — flatten `PatchMatrix`, drop `Layer` enum
priority: high
created: 2026-06-09
epic: E004
---

## Summary

Second ticket of [E004](../../epics/open/E004-single-layer-collapse.md).
Removes per-layer storage from the matrix surface. After this ticket
there is one `MatrixTable` per patch — no `Layer` enum, no `PatchMatrix`
wrapper.

Per [ADR 0002](../../adrs/0002-drop-dual-layer.md): the matrix slot
count (16), source set, dest set, and per-slot smoothing are unchanged.
Only the upper/lower duplication goes.

## Acceptance criteria

- [ ] `vxn2_engine::matrix::PatchMatrix` struct deleted (currently
      `{ upper: MatrixTable, lower: MatrixTable }`). Replaced by a
      single `MatrixTable` carried directly on the engine (or on
      `SharedParams`, whichever 0033 left it on).
- [ ] `PatchMatrix::table()`, `PatchMatrix::table_mut()` methods deleted.
- [ ] `vxn2_engine::matrix::Layer` enum deleted. All `use matrix::Layer`
      imports removed.
- [ ] Any callsite that took `&Layer` collapses to no argument.
- [ ] `SharedParams.mtx_depths: [[f32; 8]; 2]` → `mtx_depths: [f32; 8]`.
      Indexing call sites (engine snapshot, app controller) updated.
- [ ] `vxn2-engine` matrix unit tests updated: the per-layer tests at
      `matrix.rs:899-903` (asserting upper vs lower routing) are
      deleted. The remaining tests run against the single table.
- [ ] `cargo build -p vxn2-engine` green.
- [ ] `cargo test -p vxn2-engine` green.
- [ ] No `Layer::Upper` / `Layer::Lower` referenced anywhere in
      `crates/vxn2-engine/`.

## Notes

This ticket sequences after 0033 — the engine `Patch` is already flat
before this PR opens. If 0033 left a transient `matrix.upper`
reference in `Engine::process_block`, this ticket renames that to a
plain `matrix` field and deletes the `.upper` access.

The matrix slot struct itself (`MatrixSlot { source, dest, depth,
curve }`) is unchanged. The slot *count* is unchanged (16 per patch
plus the existing slot 1–8 CLAP-depth exposure rule). Only the
duplication-by-layer goes.

The mod matrix overlay JS panel (`assets/panels/mod-matrix.js`) carries
its own per-layer logic — that is **0038**, not this ticket. This
ticket stops at the Rust boundary.

The `SharedParams` field `mtx_depths` is currently the only piece of
the per-layer storage that lives on the parameter side rather than the
matrix side. After this ticket it collapses to a flat slice; the CLAP
shell (**0036**) will then map its 8 depth params 1:1 into it.

The factory preset format (not yet authored — see VXN1 ADR 0005 for the
file shape) will reference the flat matrix; no migration shim is needed
because no factory bank exists yet.
