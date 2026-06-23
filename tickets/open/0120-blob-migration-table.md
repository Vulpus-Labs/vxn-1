---
id: "0120"
product: vxn-2
title: vxn2-engine — data-driven blob-migration table + section const-asserts
priority: high
created: 2026-06-23
epic: E027
---

## Summary

`vxn2-engine/src/shared.rs` carries the host-state blob codec.
It now has 14 blob versions with a 188-line ladder of seed
blocks (`shared.rs:158-214` + `load_bytes:803-991`). Adding
one appended param requires **five coordinated edits**: a new
`LEGACY_VN_PARAM_COUNT`, a new `N_*_PARAMS_VN` count, an
`if version <= N` seed block, a `BLOB_VERSION` bump, and a
docstring line — with zero compile-time check that they
agree. A missed seed block silently loads stale store
contents into the new slot. This is the #1 param-velocity
tax in vxn-2.

Separately, `snapshot_from` (`shared.rs:1290-1380`) does ~11
offset reads like `shared.get(pb + OFF_DELAY + 1)` with no
link between the `OFF_*` constant and the descriptor it
points at; inserting a param mid-section silently shifts
every later offset.

## Acceptance criteria

- [ ] The per-version seed blocks are replaced by a
      data-driven table — `&[(version_added: u16, ids:
      Range<usize>)]` — iterated once during `load_bytes`.
      Adding an appended param becomes one table row, not
      five edits. The op-block spread migrations (v6, v11)
      keep their dedicated `migrate_*` fns but are dispatched
      from the same table.
- [ ] Each param section gets a `const`-assert that its
      `OFF_*` offset still resolves to the expected
      descriptor id, e.g.
      `const _: () = assert!(eq(PARAMS[PATCH_BASE +
      OFF_DELAY].id, "delay-on"));` — a mid-section insert
      then fails to compile. Zero runtime cost.
- [ ] A round-trip test loads a recorded blob of **every**
      historical version `1..=BLOB_VERSION` and asserts the
      resulting store contents are byte-identical before and
      after this rewrite (capture the "before" vectors from
      current `main` first).
- [ ] `cargo test -p vxn2-engine` green; the codec change
      alters no rendered audio (baseline hash unchanged).

## Notes

This is the one ticket in E026 that touches a persistence
codec — get the round-trip oracle in place **before**
rewriting, so any divergence is caught. Do not change
`BLOB_VERSION` numbering or the on-disk byte order; only the
in-code migration mechanism changes. The param table is
append-only by design (memory `vxn1-id-stability-dropped`
applies to vxn-1 only — vxn-2 keeps append discipline); the
const-asserts enforce it mechanically for the first time.
