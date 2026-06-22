---
id: "0078"
product: vxn-1
title: vxn-clap — use core batch_range, extract diff_params, dedup display/type-in
priority: medium
created: 2026-06-21
epic: E024
---

## Summary

Three smaller dedup/extraction items in the vxn-clap shell,
all in `vxn-clap/src/lib.rs`. None is large; together they
move the trickiest untested host-boundary logic onto tested,
pure code.

1. `VxnAudioProcessor::process` re-implements
   `vxn_core_clap::batch_range` inline
   (`vxn-clap/src/lib.rs:353-365`). The
   `Bound::{Included,Excluded,Unbounded}` → `(start,end)`
   frame math (with the `Excluded(n)=>n+1` /
   `Included(n)=>n+1` asymmetry) is exactly the kind of
   off-by-one you write a tested helper for and never
   hand-copy. The crate already has the helper.

2. `VxnMainThread::push_param_diffs`
   (`vxn-clap/src/lib.rs:193-236`) is a god-function
   tangling NaN-aware change detection, sync-aware display
   computation, and the "sync flip forces rate-partner
   refresh" business rule — all welded to the live
   `EditorHandle`. It is the only path surfacing audio-thread
   automation to the UI, the sync-partner refresh is a known
   foot-gun, and it has zero test coverage.

3. `value_to_text` (`:496-518`) re-implements the
   sync-partner-check-then-label logic that
   `sync_aware_display` (`:447-457`) already encapsulates;
   `text_to_value` (`:520-529`) parses a leading numeric run
   and ignores `ParamDesc::variant_index`, so host type-in
   of an enum label silently fails.

## Acceptance criteria

- [ ] `process` calls `vxn_core_clap::batch_range(
      event_batch.sample_bounds(), frames)`; the inline
      conversion at `lib.rs:353-365` is gone.
- [ ] The diff/sync-partner logic in `push_param_diffs` is
      extracted to a pure function — e.g. `fn diff_params(
      model: &impl ParamModel, last_seen: &mut [f32],
      sync_partner: impl Fn(usize)->Option<usize>) ->
      Vec<ViewEvent>` — living in `vxn-app` (next to
      `sync.rs`) or `vxn-core-clap` if vxn-2 can share it.
      `push_param_diffs` becomes "call the pure fn, push
      results into the handle."
- [ ] The pure fn has unit tests covering: a plain value
      change, a NaN/no-change skip, and a sync flip forcing
      the rate-partner refresh (the second collect-then-emit
      pass) — currently uncovered.
- [ ] `value_to_text` calls `sync_aware_display(&self.shared
      .params, id, value as f32)` rather than re-deriving the
      0.5-threshold + `synced_label_for` rule.
- [ ] `text_to_value` routes through `ParamDesc`
      (`variant_index` for enum/bool params, range-clamp for
      floats) instead of leading-numeric-run parsing; if
      extracted as a free fn it gains a unit test for an enum
      label and an out-of-range float.
- [ ] `cargo test --workspace` green; `tests/baseline.rs`
      hash unchanged (none of this touches the render path).

## Notes

Depends on E011 **0017** landing the shared
`vxn_core_clap::LocalParams<N>` — once vxn-clap consumes the
shared mirror, the `last_seen`/`host_changed` accessors the
pure `diff_params` needs are already on it. 0017's Notes
explicitly scoped vxn-2's `batch_range` dedup to vxn-2 E012;
this ticket is the vxn-1 counterpart, not covered there.

## Close-out (2026-06-22)

- `process` now calls
  [vxn_core_clap::batch_range](../../vxn-1/crates/vxn-clap/src/lib.rs#L341);
  the inline `Bound` → `(start,end)` conversion is gone.
- Diff/sync-partner logic extracted to pure
  [diff_params](../../vxn-1/crates/vxn-app/src/diff.rs#L25) in vxn-app
  (next to `sync.rs`). `push_param_diffs`
  ([lib.rs](../../vxn-1/crates/vxn-clap/src/lib.rs#L188)) is now "call the
  pure fn, fan events into the handle." Kept in vxn-app (not vxn-core-clap)
  — the sync rules + `desc_for_clap_id` table it depends on are vxn-1's.
- Unit tests cover all three required paths:
  `diff::tests::plain_value_change_emits_one_event`,
  `diff::tests::no_change_skips_and_nan_seed_broadcasts` (NaN seed forces
  broadcast; equal values skip), and
  `diff::tests::sync_flip_forces_rate_partner_refresh` (the second
  collect-then-emit pass, asserting the rate partner re-emits with its
  synced label even though its value didn't move).
- `sync_aware_display` moved into
  [vxn-app sync.rs](../../vxn-1/crates/vxn-app/src/sync.rs#L118) (generic
  over `ParamModel`); `value_to_text`
  ([lib.rs](../../vxn-1/crates/vxn-clap/src/lib.rs#L432)) now calls it
  instead of re-deriving the 0.5-threshold + `synced_label_for` rule.
- `text_to_value` routes through new
  [ParamDesc::parse](../../crates/vxn-core-app/src/params.rs#L121)
  (variant label for enum/bool, range-clamp for float/int) instead of
  leading-numeric-run parsing — host type-in of an enum label ("Saw") /
  bool ("On") now works. Covered by
  `params::tests::parse_enum_label_and_out_of_range_float` and
  `params::tests::parse_bool_accepts_labels_and_numbers`.
- `cargo test --workspace` green; `tests/baseline.rs`
  `baseline_render_is_stable` unchanged (no render-path touch).
