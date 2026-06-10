---
id: "0066"
title: "value_to_text routes through sync_aware_display"
priority: medium
created: 2026-06-10
epic: E006
depends: []
---

## Summary

Sixth ticket of [E006](../../epics/open/E006-review-remediation.md).
The CLAP `value_to_text` path calls `format_value` → `desc.display`
unconditionally
([lib.rs:574-578](../../crates/vxn2-clap/src/lib.rs#L574)), while the
view pump uses `sync_aware_display`. Result: with `lfo1-sync` (or
`delay-sync`) on, the editor shows `1/8` but the host's automation
lane / generic param UI shows the raw Hz/ms value. The stale comment
at the same site ("sync-aware substitution … slots in here in the UI
epic") marks the spot — the UI epic shipped the helper but never
swapped this call site.

## Fix

`value_to_text` consults the same `sync_pairs()` table: if the queried
id is a rate param whose sync partner is currently on, format via
`sync_aware_display`; otherwise `desc.display` as today. Delete the
stale comment.

Reading the sync partner's current value inside `value_to_text` (a
main-thread host call) from `SharedParams` is the same pattern
`drain_dirty_bits` already uses — no new threading concerns.

## Acceptance criteria

- [ ] Unit test: `value_to_text` for `lfo1-rate` with `lfo1-sync` on
  returns a subdivision label; with sync off returns Hz. Same for the
  delay pair (and any third pair in `sync_pairs()`).
- [ ] `text_to_value` round-trip considered: either accept subdivision
  strings when sync is on, or document that text input remains
  Hz-only (acceptable; note it in the test).
- [ ] Stale comment at lib.rs:574-578 removed.

## Notes

Review also flagged that `sync_pairs` / `sync_aware_display` are
domain logic stranded in the CLAP shell and would be duplicated by a
future non-CLAP frontend. Moving them to `vxn2-engine` (next to
`ParamDesc`) is in-scope here if the move is mechanical; if it snags,
leave a `// TODO(E006)` and move on — the display fix is the point.

## Close-out (2026-06-10)

- `value_to_text` now writes `sync_aware_display(...)` — same path as
  the view pump, so host automation lanes and the editor agree on
  subdivision labels. Stale "slots in here in the UI epic" comment and
  the `format_value` shim deleted.
- `text_to_value` stays Hz/ms-only, documented at the function: a
  subdivision string ("1/8") parses as 1.0 via the leading-numeric
  token rule; hosts use value_to_text for display and pass plain
  values for edits.
- The optional move landed: `sync_pairs` / `sync_partner_clap_id` /
  `rate_partner_clap_id` / `sync_aware_display` now live in
  `vxn2_engine::sync` (re-exported at the crate root); `vxn2-clap`
  imports them. Per-pair unit tests (lfo1, delay, lfo2 + unpaired
  fallthrough) live next to the moved code.
- `value_to_text` itself is untestable off-host (`ParamDisplayWriter`
  has no public constructor); the seam under test is
  `sync_aware_display`, which the method now delegates to verbatim.
