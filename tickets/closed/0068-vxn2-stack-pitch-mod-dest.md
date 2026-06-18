---
id: "0068"
product: vxn-2
title: "Op-N stack-pitch mod destination(s) + wire/blob migration"
priority: medium
created: 2026-06-18
epic: E022
depends: ["0067"]
---

## Summary

Add the mod-matrix **destination surface** for stack pitch mod, so a route
can name "Op N stack pitch" as a target. Inert until 0069 wires the cook —
this ticket is the enum + wire-format + migration plumbing only.

## Design

- **Six new dests**, `Op1StackPitch..Op6StackPitch`, mirroring the existing
  `Op1Pitch..Op6Pitch` block in
  [matrix.rs](../../vxn-2/crates/vxn2-engine/src/matrix.rs#L348). Encoding
  the target op in the dest enum keeps routes as plain `(source, dest,
  depth)` — no extra per-route param.
  - Alternative considered: a single `StackPitch` dest + a separate
    target-op selector. Rejected — routes have no spare field, and 6 dests
    matches the existing per-op pattern. Note the trade-off in close-out if
    you deviate.
- **Append at the end** of `DestId`, after `Resonance`, so the blob
  migration stays a 1:1 prefix (same discipline as the filter dests in
  [shared.rs](../../vxn-2/crates/vxn2-engine/src/shared.rs#L99) v-migrations).
  `N_DESTS` 29 → 35.
- **Tier = per-lane** (same as `OpNPitch`) — pitch is a per-lane dest.
- Update `from_u8` decode, `DEST_NAMES` (kebab wire names, e.g.
  `op1-stack-pitch`), `DEST_LABELS` (display, e.g. "Op 1 Stack Pitch"), and
  the `coherence()` table (stack-pitch coheres like per-op pitch — sub-block
  pitch-smoothed, see the matrix module's `PitchSmoother` note).
- **Blob version bump** for the widened dest space; add the migration step
  (old blobs decode unchanged since dests are appended). Mirror the existing
  numbered migration comments.

## Acceptance criteria

- [ ] `Op1StackPitch..Op6StackPitch` added to `DestId`, appended after
      `Resonance`; `N_DESTS == 35`.
- [ ] `from_u8`, `DEST_NAMES`, `DEST_LABELS`, `coherence()`, and any
      `idx()`/range asserts updated; existing dest indices unchanged.
- [ ] Blob version bumped; an old (pre-0068) preset/state blob still loads
      and round-trips (the new dests default to unused).
- [ ] The matrix eval treats these dests as inert (zero effect) until 0069 —
      a route to a stack-pitch dest is a no-op, not a panic.
- [ ] Param-count / matrix-surface tests in `shared.rs` updated to the new
      totals.

## Notes

- No resolver call here — this ticket only makes the target *nameable* and
  persistable. The scatter that gives it effect is 0069.
- Watch the `assert_eq!(DestId::Feedback.idx(), Some(N_DESTS - 3))`-style
  invariants in the matrix tests — they shift with the appended dests.

## Close-out (2026-06-18)

- `Op1StackPitch..Op6StackPitch` appended to `DestId` after `Resonance`;
  `N_DESTS` 29 → 35 ([matrix.rs](../../vxn-2/crates/vxn2-engine/src/matrix.rs)).
  Existing dest indices unchanged (`Resonance.idx()` still 28).
- `from_u8` (30..=35), `DEST_NAMES` (`opN-stack-pitch`), `DEST_LABELS`
  (`Op N Stack Pitch`), `DEST_GAIN` (24.0), `tier()` (PerLane), `cook_depth`
  (cubic taper) all updated; `coherence()` needs no special case — PerLane tier
  makes the generic rule treat them exactly like per-op pitch
  (`stack_pitch_dests_cohere_like_per_op_pitch`).
- Inert until 0069: a stack-pitch route writes only its own accumulator column,
  touches no per-op pitch, no panic (`stack_pitch_route_evals_inert_no_panic`).
- Blob `BLOB_VERSION` 9 → 10 ([shared.rs](../../vxn-2/crates/vxn2-engine/src/shared.rs#L131)).
  Byte layout/param count unchanged (dest is a `u8` in the matrix trailer); the
  bump is a forward-compat guard so a pre-E022 build rejects a stack-pitch patch
  rather than silently dropping the route. Old blobs decode 1:1
  (`snapshot_round_trips_stack_pitch_route`, existing v≤9 migration tests pass).
- Index/round-trip invariants updated (`dest_idx_skips_none_and_packs_others`)
  and the data-driven UI dest count test (ui-web
  `build_matrix_lists_json_includes_all_enum_widths`, 30 → 36).
