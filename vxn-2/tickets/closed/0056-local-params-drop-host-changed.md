---
id: "0056"
title: "LocalParams: drop host_changed, audio writes go through dirty bitset"
priority: high
created: 2026-06-10
epic: E005
depends: ["0055"]
---

## Summary

Second ticket of [E005](../../epics/open/E005-dirty-bitset-pump.md).
With the dirty bitset on `SharedParams` ([0055](./0055-shared-params-dirty-bitset.md)),
the audio-thread `LocalParams` no longer needs its own change-tracking
array for Model → View notification. Delete `host_changed`; audio writes
go through to the shared store and flip dirty bits like any other write
site. The pump catches it on the next tick.

`ui_changed` survives — different consumer (plugin → host gesture
brackets, ADR 0003 §"What survives"). Different ticket if/when it
follows the same pattern.

## Acceptance criteria

- [ ] `LocalParams.host_changed: [bool; TOTAL_PARAMS]` field deleted.
- [ ] `LocalParams::apply_input(event)` no longer sets a `host_changed`
  flag. Writes the local mirror (`self.values[i] = v`) and either:
  - Also writes through to `SharedParams.set(i, v)` immediately
    (cleanest — one path, one bit flip), OR
  - Defers to a slimmer `publish()` that only writes-through (no
    flag walk).
  Pick one and document. The "write through on apply" option removes
  `publish()` entirely; the deferred option keeps it as a no-flag
  fanout. The "write through on apply" path is preferred — fewer
  moving parts, one less per-block walk.
- [ ] `LocalParams::publish` either deleted or reduced to a no-op /
  doc-only stub depending on the choice above. Caller in
  `vxn2-clap` `process()` updated accordingly.
- [ ] `LocalParams::fetch_ui_changes` still walks `SharedParams.values`
  + `matrix_meta` + `matrix_extra_depth` to refresh the mirror. (The
  audio-thread mirror exists for hot-loop cheap reads, not change
  tracking.) The shared store is the source of truth; the mirror
  catches up at the top of each block. Unchanged.
- [ ] `LocalParams::ui_changed` stays. `emit()` still walks it for
  plugin → host events. (Separate concern; see ADR 0003 §"Open
  questions" for the follow-up that gives `ui_changed` the same
  bitset treatment.)
- [ ] Audio-thread `apply_input` test in `local.rs` updated: the
  expectation is no longer "host_changed[id] is set", it's "shared
  store now contains the new value" (or "publish wrote it through" —
  depending on the chosen path).
- [ ] `cargo build -p vxn2-clap` green.
- [ ] `cargo test -p vxn2-clap -p vxn2-engine` green.
- [ ] No regression in the existing `process_loop_*` tests in
  `vxn2-clap` — the audio thread → shared store fanout still happens,
  just without a flag array.

## Notes

This ticket isolates the audio-thread side from the main-thread pump.
Once 0055 + 0056 land, **every** write site to `SharedParams` flips a
dirty bit. The pump (0057) can rely on that contract.

Risk: if the chosen path is "write through on apply", every host
event now does two atomic stores (local mirror + shared values) plus a
`fetch_or` on the bitset, where today it's one local-array assignment
and a deferred bulk publish. Profile if Reaper-style heavy automation
is a concern. The bulk publish saved one atomic store per slot at the
cost of the flag-walk loop; for typical loads (a few automated params)
the per-write approach is cheaper and simpler.

If the bulk-publish path is kept (apply sets a flag, publish does the
atomic stores + bit flips), it's still a clean simplification — the
flag is internal book-keeping, not a Model → View signal. Document why.
