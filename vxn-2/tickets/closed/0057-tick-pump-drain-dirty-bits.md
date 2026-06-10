---
id: "0057"
title: "Main-thread tick: replace collect_param_diffs with drain_dirty_bits"
priority: high
created: 2026-06-10
epic: E005
depends: ["0055"]
---

## Summary

Third ticket of [E005](../../epics/open/E005-dirty-bitset-pump.md).
Replace the polling diff in `collect_param_diffs` and the `last_seen`
shadow snapshot with a bitset-driven pump that reads
`SharedParams.dirty_values` + `dirty_matrix` via swap-and-walk. This is
the single Model → View bridge after the refactor.

Per [ADR 0003](../../adrs/0003-dirty-bitset-diff-pump.md): popcount
over set bits, atomic `swap(Acquire)` clears the word, `get(id)` reads
the latest value. Whole-table `MatrixSnapshot` push if any matrix bit
was set.

## Acceptance criteria

- [ ] `VxnMainThread.last_seen: Vec<f32>` field deleted.
  Constructor calls in `mk_main` etc. drop the
  `last_seen: vec![f32::NAN; TOTAL_PARAMS]` arg.
- [ ] `collect_param_diffs(params, last_seen)` deleted. Replaced with
  `drain_dirty_bits(params) -> Vec<ViewEvent>` (or in-place push to a
  buffer — pick whichever matches the editor handle's API best).
- [ ] `drain_dirty_bits`:
  - Calls `SharedParams::take_dirty_values()` and walks set bits.
    For each id, pushes one `ParamChanged { id, plain, normalised,
    display }`. Skip ids past `TOTAL_PARAMS` (defensive — bitset
    width should already match, but the popcount walk handles trailing
    bits gracefully).
  - Calls `SharedParams::take_dirty_matrix()`. If non-zero, pushes
    one `MatrixSnapshot` carrying all 16 rows
    (`vxn2_app::push_matrix_snapshot` semantics — refactor to share
    code or inline).
- [ ] Sync-pair re-emit preserved: if `lfo1-sync`, `delay-sync`, or
  `lfo2-sync` is in the popped value bits, also emit a fresh
  `ParamChanged` for its rate partner so the display switches
  representation. Same logic as in `collect_param_diffs`, just hooked
  off the bitset walk instead of the polling diff.
- [ ] `push_param_diffs` is renamed to `push_model_diffs` (or
  `drain_dirty_to_view`) and now also drains matrix. Updates the
  call site in `PluginTimerImpl::on_timer`.
- [ ] `VxnMainThread`'s timer tick path stays the same shape: one
  drain call per tick → events buffer → editor handle flush.
- [ ] Existing unit tests in `vxn2-clap` that target
  `collect_param_diffs` migrate to `drain_dirty_bits`. New
  expectations:
  - Writing one id then draining returns one `ParamChanged` for that
    id.
  - Writing the same id 5 times between drains returns one
    `ParamChanged` carrying the latest value (coalescing).
  - Writing a matrix slot meta returns one `MatrixSnapshot`, not 16
    row events.
  - First drain after `SharedParams::new` returns the full table
    (initial "all-ones" bitset, parallels NaN-seed today).
  - Second drain with no intervening writes returns empty.
  - Sync-flag flip emits the rate-partner display.
- [ ] No `param_changed` event ships for ids that are mid-gesture
  according to `SharedParams.gestures`. (Same gate as today's
  `collect_param_diffs` — that part isn't moving; see notes.)
- [ ] `cargo build -p vxn2-clap` green.
- [ ] `cargo test -p vxn2-clap` green.

## Notes

The `gestures` bitset gate stays in this pump for backwards
compatibility with the current contract: mid-drag, the host's
echo-back doesn't fight the UI. ADR 0003 marks this for a follow-up —
ideally gesture suppression migrates to the view layer
(`bindGestureGated` in 0060), and the pump becomes truly dumb. Until
0060 lands and the bind helper is everywhere, keeping the gate here
is the safer transition. Document the temporary duplication in the
PR.

Performance: the polling diff is O(N_PARAMS) per tick. The bitset
walk is O(popcount × N_PARAMS_WORDS) — typically far cheaper for
incremental UI workload, comparable for full-table broadcast.

Test fixture: a small helper `assert_drains_just(params, expected_ids)`
that calls `drain_dirty_bits` and checks the emitted `ParamChanged`
ids against the expected set. Many tests get cleaner.
