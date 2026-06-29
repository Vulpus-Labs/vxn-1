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

## Close-out (2026-06-29)

- `BLOB_VERSION` reset 16 → 1; `load_bytes` now rejects any
  `version != BLOB_VERSION` with `UnsupportedVersion`
  ([shared.rs:819](../../vxn-2/crates/vxn2-engine/src/shared.rs#L819),
  [shared.rs:112](../../vxn-2/crates/vxn2-engine/src/shared.rs#L112)).
  Ladder collapsed to a single current-version path: value block 1:1,
  then matrix / KS / EG trailers straight-read, no remap, no seeding.
- Deleted every ladder artefact — 3 `migrate_v*_id` fns, all
  `LEGACY_V*_PARAM_COUNT` / `N_*_PARAMS_V*` consts,
  `LIVE_RATIO_MODE_IDX` / `LIVE_PHASE_IDX` / `LEGACY_AMP_SENS_IDX` /
  `LEGACY_LFO1_DEPTH_ID`, every per-version seed block, the version-keyed
  `expected_count` / `expected` match arms, the v2→v3 dest remap and
  `DestId::from_u8_v2` ([matrix.rs](../../vxn-2/crates/vxn2-engine/src/matrix.rs)).
  Net: shared.rs −767, matrix.rs −39. Grep sweep for
  `migrate_v|LEGACY_V|from_u8_v2|_PARAM_COUNT` over the engine crate → none.
- 34 section-offset compile guards added: `const _: () =
  assert!(id_eq(PARAMS[…].id, "…"))` pinning each `OFF_*` anchor + section
  width (op-block anchor/stride/trailing slots, every per-patch section,
  all 9 patch-level sections)
  ([shared.rs:120+](../../vxn-2/crates/vxn2-engine/src/shared.rs#L120)).
  A mid-section insert now fails to compile; zero runtime cost.
- Round-trip test retained
  (`shared::tests::snapshot_bytes_round_trip_is_bit_identical`); added
  `shared::tests::load_bytes_rejects_old_version` (v15 → `UnsupportedVersion`).
  Removed all `rewrite_as_*` helpers + `load_bytes_migrates_*` +
  `v*_blob_seeds_*` + `accepts_legacy_v1` migration tests.
- `cargo test -p vxn2-engine` green (lib 202 passed; param/sweep/zipper
  suites pass); `tests/baseline.rs::render_hash_unchanged` ok — codec change
  alters no rendered audio.
