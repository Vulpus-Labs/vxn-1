---
id: "0120"
product: vxn-2
title: vxn2-engine — drop blob migration ladder, add section const-asserts
priority: high
created: 2026-06-23
epic: E027
---

## Summary

`vxn2-engine/src/shared.rs` carries the host-state blob codec
with a 14-version migration ladder: per-version seed blocks
(`shared.rs:158-214`), `migrate_v4/v5/v10_id` id-remap fns, a
stack of `LEGACY_V*_PARAM_COUNT` consts, and version-keyed
`expected_count` / `expected` length matches in `load_bytes`
(`shared.rs:919-999+`). Adding one appended param requires
**five coordinated edits** with zero compile-time check that
they agree — the #1 param-velocity tax in vxn-2.

vxn-2 has **no users** and carries **no blob back-compat
obligation until ≥1.0.0** (per maintainer). So the entire
ladder is dead weight: there are no historical blobs in the
wild to migrate. Delete it.

Separately, `snapshot_from` (`shared.rs:1290-1380`) does ~11
offset reads like `shared.get(pb + OFF_DELAY + 1)` with no
link between the `OFF_*` constant and the descriptor it
points at; inserting a param mid-section silently shifts
every later offset. That guard is independent of back-compat
and stays in scope.

## Acceptance criteria

- [ ] `load_bytes` accepts only `version == BLOB_VERSION`;
      any other version returns `UnsupportedVersion`. Reset
      `BLOB_VERSION = 1`. Delete every `migrate_v*_id` fn,
      every `LEGACY_V*_PARAM_COUNT` / `N_*_PARAMS_V*` const,
      every per-version seed block, and the version-keyed
      `expected_count` / `expected` match arms — collapse to
      the single current-version path.
- [ ] Each param section gets a `const`-assert that its
      `OFF_*` offset still resolves to the expected
      descriptor id, e.g.
      `const _: () = assert!(eq(PARAMS[PATCH_BASE +
      OFF_DELAY].id, "delay-on"));` — a mid-section insert
      then fails to compile. Zero runtime cost.
- [ ] A snapshot round-trip test: `snapshot_bytes` →
      `load_bytes` reloads byte-identical store contents
      (current version only — no historical blobs).
- [ ] `cargo test -p vxn2-engine` green; the codec change
      alters no rendered audio (baseline hash unchanged).

## Notes

Existing dev/test blobs at any prior version become
unreadable after this — re-save from the live build. That is
acceptable pre-1.0.0 and is the whole point of the cut.

The data-driven migration table and 14-version round-trip
oracle from the original 0120 are **dropped**: with the
ladder deleted there are no migrations left to table-drive
and no historical versions to round-trip. Only the
const-asserts (which enforce append discipline mechanically
for the first time) survive from the original scope.

The param table stays append-only by design (memory
`vxn1-id-stability-dropped` applies to vxn-1 only); the
const-asserts enforce it at compile time. The next time
real back-compat is needed (≥1.0.0), reintroduce a versioned
migration mechanism then — not before.
