---
id: "0303"
product: vxn-1b
title: "vxn1b-web-controller: drain bits, delete 0290's explicit broadcasts"
priority: medium
created: 2026-08-25
epic: E046
depends: ["0301", "0290"]
---

## Summary

Ticket of [E046](../../epics/open/E046-dirty-bitset-pump-vxn1-vxn1b.md): the web
half of VXN1b moves onto [[0301]]'s bits, and four workarounds
[0290](0290-vxn1b-web-controller-cdylib.md) had to invent go away.

**Depends on 0290 shipping first.** This is not "0290 done differently" — the web
controller has to exist and work before it is rewired, and 0290 is not blocked on
E046.

## What 0290 had to hand-build, and what replaces it

0290's design review found three model writes with no notify path and one
missing display rule. Each has a bits-shaped answer:

| 0290 workaround | replaced by |
|---|---|
| explicit `broadcast_all_params()` after `vxnc_restore_state` | `restore_from_bytes` marks the table ([[0301]]) |
| explicit `broadcast_all_params()` after `vxnc_import_toml` | ditto |
| explicit `broadcast_all_params()` after `PatchOp::CopyLayer` | `copy_layer` marks params + matrix + key ([[0301]]) |
| pack-time `sync_aware_display` recompute + synthesised rate-partner records | the drain emits them directly, as vxn-2's does ([lib.rs:38-89](../../vxn-2/crates/vxn2-web-controller/src/lib.rs#L38-L89)) |
| the matrix + key memo diffs ported from `vxn1b-clap` | the matrix / key dirty channels |

After this the file converges on vxn-2's shape, which is the point: two web
controllers with the same change-detection story instead of two dialects.

## Design

- `tick()` drains `view_rx` (now non-param events only) then the model's bits,
  packing `ParamChanged` / `MatrixSnapshot` / `KeyState` records. Structurally
  vxn-2's `drain_dirty_bits`, minus the KS/EG curve arms, using [[0299]]'s
  callback drain so it does not allocate per tick.
- `set_echo_param_writes(false)`.
- `vxnc_ui_request_full_rebroadcast` becomes implementable the way vxn-2 does it
  ([lib.rs:803](../../vxn-2/crates/vxn2-web-controller/src/lib.rs#L803)) — a
  `mark_all()` — which is cheaper than `EditorReady`'s broadcast and covers the
  non-param state too. Decide whether to keep `EditorReady`'s path as well or
  route both through the mark.
- Delete the memo fields and the explicit broadcast calls.

There is still no host and no readback ([[0297]] removed the SAB region) — that
does not change here. The difference is that the *controller's own* direct writes
now announce themselves, which is what was missing.

## Acceptance criteria

- [ ] `drain_dirty_bits` equivalent exists; `echo_param_writes(false)`; a test
      asserts one `ParamChanged` per UI write.
- [ ] `vxnc_restore_state`, `vxnc_import_toml` and `copy_layer` each re-broadcast
      with **no explicit broadcast call** in the controller — pinned by a test
      per path, since these are the three that silently emitted nothing before
      0290's workaround.
- [ ] Sync-aware displays and rate-partner refresh come from the drain, and the
      pack-time recompute is deleted.
- [ ] Matrix topology and key state reach the page after a preset load, a state
      restore and a layer copy, with no memo fields left in `ControllerState`.
- [ ] `vxnc_total_params()` still agrees with the engine.
- [ ] `cargo test -p vxn1b-web-controller` green; the VXN1b web node suite green,
      0 skipped ([[0295]]'s posture).
- [ ] Browser pass: load a preset, copy Layer 1 → 2, import a patch — the
      faceplate repaints fully in each case.

## Notes

- If 0290 shipped its workarounds with comments pointing here, delete the
  comments too — a stale "until E046" note is worse than none.
- The demo posture of [[0297]] is unchanged by this ticket: it removes code, adds
  no durability.
- One `cargo test` at a time — [[vxn-no-parallel-cargo-test]]. No `cargo fmt` —
  [[vxn-no-cargo-fmt]]. Stage explicit paths —
  [[vxn-concurrent-vxn2-work-no-git-add-all]].
