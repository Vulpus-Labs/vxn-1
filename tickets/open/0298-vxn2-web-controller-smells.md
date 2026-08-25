---
id: "0298"
product: vxn-2
title: "vxn2-web-controller: review the seven smells the VXN1b port review surfaced"
priority: low
created: 2026-08-25
epic: E030
depends: []
---

## Summary

Reviewing `vxn2-web-controller` as the source for VXN1b's port
([0290](0290-vxn1b-web-controller-cdylib.md)) turned up seven defects and smells
in vxn-2 itself. None is user-visible today — the shipped web build works — so
this is a **review-and-decide** ticket, not a fix-list: each item below needs a
verdict (fix / accept-and-document / not-a-bug), and only the ones that survive
that get changed.

Two of them (1 and 2) are the same underlying issue and are the reason to open
this at all: the double-emission is load-bearing on an ordering nobody wrote
down and no test pins.

## The findings

### 1. Preset load emits every param twice

[`load_preset`](../../crates/vxn-core-app/src/controller.rs#L435) calls
`broadcast_all_params()` unconditionally, *and*
[`restore_from_bytes`](../../vxn-2/crates/vxn2-engine/src/shared.rs#L967) marks the
whole table dirty (pinned by
[`load_bytes_marks_full_table_dirty`](../../vxn-2/crates/vxn2-engine/src/shared.rs#L2083)),
so [`drain_dirty_bits`](../../vxn-2/crates/vxn2-web-controller/src/lib.rs#L38)
re-emits all 209 in the same tick. ~418 records and 209 wasted display-string
allocations per preset load.

`HostEvent::StateLoaded` gates its broadcast on `echo_param_writes` for exactly
this reason ([controller.rs:400-405](../../crates/vxn-core-app/src/controller.rs#L400-L405),
ticket 0067); `load_preset` was never given the same gate. Note this is a
**shared-crate** change — vxn-1 and VXN1b run the same `load_preset` with echo
on, where the broadcast is the only emitter and must stay.

### 2. …and the duplicate pair disagrees about the display string

The broadcast copy carries `descriptor.display()`
([controller.rs:514-528](../../crates/vxn-core-app/src/controller.rs#L514-L528));
the bitset copy carries `sync_aware_display`
([lib.rs:81](../../vxn-2/crates/vxn2-web-controller/src/lib.rs#L81)). Both land in
one batch and it renders correctly **only** because the bitset drain runs after
the `view_rx` drain ([lib.rs:559](../../vxn-2/crates/vxn2-web-controller/src/lib.rs#L559))
and the JS keeps the last record per id.

Swap those two loops, or change the JS to first-wins, and every synced LFO rate
and delay time silently reverts to raw Hz/ms after a preset load. Nothing tests
this. Fixing (1) also fixes (2); if (1) is declined, this wants a test that
pins the ordering, or a comment saying it is load-bearing.

### 3. `export_toml` drops the loaded preset's metadata

[lib.rs:605](../../vxn-2/crates/vxn2-web-controller/src/lib.rs#L605) builds
`PresetMeta { name, ..Default::default() }`, so an exported patch loses author /
category / comment even when the current patch came from a preset carrying them.
Re-importing files it under "Uncategorized". The controller knows the live
source (`current_source`); the export doesn't ask.

### 4. `delete_folder` succeeds on a folder that doesn't exist

[user_store.rs:296-311](../../vxn-2/crates/vxn2-web-controller/src/user_store.rs#L296-L311)
returns `Ok(())` and journals a `DeleteFolder` regardless, while
[`rename_folder`](../../vxn-2/crates/vxn2-web-controller/src/user_store.rs#L261)
returns `"folder not found"`. Asymmetric error contract — a double-fire from the
UI reports success twice and writes a redundant IndexedDB op.

### 5. `hydrate_preset` trusts the key's slash structure

[user_store.rs:113](../../vxn-2/crates/vxn2-web-controller/src/user_store.rs#L113)
derives the folder with `rsplit_once('/')`, so a key `"a/b/c.toml"` yields folder
`"a/b"` — a value `sanitize_name` can never produce — and inserts it into the
folder set, giving a phantom nested folder in the browser tree. Hydration is the
one path with no sanitisation, because it trusts what it previously wrote;
a hand-edited or foreign IndexedDB entry breaks that assumption.

### 6. `state()` panics if `vxnc_new` wasn't called

[lib.rs:669](../../vxn-2/crates/vxn2-web-controller/src/lib.rs#L669) `expect`s. In
wasm a panic is a trap — the module is dead for the rest of the page — from what
would be a JS call-ordering mistake. The glue does guarantee the ordering, so
this may be correctly "can't happen"; the question is whether a soft no-op
return is worth it for a boot-order bug that would otherwise present as a
totally dead page.

### 7. Per-tick allocation in the drain

[`drain_dirty_bits`](../../vxn-2/crates/vxn2-web-controller/src/lib.rs#L39-L41)
allocates a `Vec<ViewEvent>` plus a `vec![false; TOTAL_PARAMS]` every tick at
~60 Hz, even when nothing is dirty. Main thread, so not RT-critical — just
needless churn on a hot loop, and reusable buffers on `ControllerState` are the
same shape as the seven that are already there.

## Acceptance criteria

- [ ] Each of the seven has a recorded verdict: fixed, accepted-and-documented,
      or not-a-bug with the reason.
- [ ] If (1) is fixed: `load_preset`'s broadcast is gated the same way
      `StateLoaded`'s is, and vxn-1 + VXN1b (echo on) still emit exactly one
      full broadcast per load — a shared-crate test covers both settings.
- [ ] If (1) is declined: a test pins the drain ordering (2) so a reorder fails
      loudly instead of degrading synced-rate labels.
- [ ] `cargo test -p vxn2-web-controller`, `cargo test -p vxn-core-app` and the
      vxn-2 web node suite stay green, 0 skipped ([[0295]]).
- [ ] No behaviour change visible in the shipped vxn-2 web build (or, if there
      is one, it is named here).

## Notes

- Source: the 0290 port review (2026-08-25), §7. Parked deliberately — VXN1b's
  port comes first, and none of these blocks it.
- Findings 1, 2 and 7 are properties of the **dirty-bitset pump**
  ([ADR 0003](../../vxn-2/adrs/0003-dirty-bitset-diff-pump.md) / [[E005]]), which
  VXN1b does not have — so they do not transfer to 0290. 3-6 are in code 0290
  *does* port, and the port should avoid reproducing them rather than wait on
  this ticket.
- Finding 1 touches `vxn-core-app`, which vxn-1, vxn-2 and VXN1b all run. Any
  change there needs all three shells re-checked, not just vxn-2's web build.
- Do **not** run more than one `cargo test` at a time — [[vxn-no-parallel-cargo-test]].
  No `cargo fmt` — [[vxn-no-cargo-fmt]]. Stage explicit paths —
  [[vxn-concurrent-vxn2-work-no-git-add-all]].
