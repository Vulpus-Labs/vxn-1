---
id: "0305"
product: vxn-1
title: "vxn-1: both shells drain bits; delete vxn-app::diff"
priority: medium
created: 2026-08-25
epic: E046
depends: ["0304", "0306"]
---

## Summary

Last ticket of [E046](../../epics/open/E046-dirty-bitset-pump-vxn1-vxn1b.md):
vxn-1's CLAP shell and web controller move onto [[0304]]'s bits, and
`vxn-app::diff` — the poll that started the whole idiom — is deleted.

## Design

### Native shell

[`push_param_diffs`](../../vxn-1/crates/vxn-clap/src/lib.rs#L216) and its
`last_seen` ([lib.rs:192](../../vxn-1/crates/vxn-clap/src/lib.rs#L192)) become a
bit drain in `on_timer`, emitting `ParamChanged` (keeping `sync_aware_display`
and the rate-partner refresh) plus `KeyModeChanged` / `SplitPointChanged` on the
non-CLAP bits. `echo_param_writes(false)`. The `on_model_loaded` republish
([controller.rs:118](../../vxn-1/crates/vxn-app/src/controller.rs#L118)) goes —
the bits cover loads and everything else. Delete the
two-pushes-can-double comment at
[lib.rs:236-239](../../vxn-1/crates/vxn-clap/src/lib.rs#L236-L239); it stops
being true.

### Web controller — sequence this carefully

**This is E046's sharpest trap.** vxn-1's browser build gets
`sync_aware_display` and the rate-partner refresh *only* from the readback pump:
`pump_readback` ([lib.rs:683](../../vxn-1/crates/vxn-web-controller/src/lib.rs#L683))
→ `nan_diff` → [diff.rs:85](../../vxn-1/crates/vxn-app/src/diff.rs#L85). The
controller's own echo carries `descriptor.display()`, which is wrong for every
synced rate and delay time. Delete the pump before the bits replace it and every
subdivision label silently becomes raw Hz.

So: add the drain, prove displays are right, *then* remove the pump — two
commits, not one.

Then decide the readback's fate on its own merits. Unlike VXN1b (where [[0297]]
removed the SAB region outright), vxn-1 keeps `rebuild()` and its readback half
still exists. Once the bits supply displays, the pump is confirmation-only —
nothing originates a param value outside the controller in a browser. Either
outcome is defensible; what is not defensible is leaving it in *and* leaving it
undocumented, since a future reader will assume it is load-bearing (it is, today,
for the display path).

### Deletions

`vxn-app::diff` in full — `nan_diff`, `diff_params`, their tests
([diff.rs](../../vxn-1/crates/vxn-app/src/diff.rs)) — plus the `pub use` at
[lib.rs:22](../../vxn-1/crates/vxn-app/src/lib.rs#L22). It has no other
consumers once both shells are converted; VXN1b never used it.

## Acceptance criteria

- [ ] Native shell drains value + key-mode + split bits; `push_param_diffs` and
      `last_seen` deleted; `echo_param_writes(false)` with a one-event test.
- [ ] Host automation from the DAW still reaches the editor.
- [ ] Key mode / split point reach the editor after a preset load, a host state
      load and a host undo, with the `on_model_loaded` republish removed.
- [ ] Web controller drains bits; **synced rate and delay-time labels still show
      subdivisions**, proven in the browser, and by a test that would fail if the
      display fell back to `descriptor.display()`.
- [ ] The readback pump's fate is recorded — kept with a comment saying it is
      confirmation-only, or removed. Not left ambiguous.
- [ ] `vxn-app::diff` is gone and nothing imports it.
- [ ] `cargo test -p vxn-clap`, `-p vxn-web-controller`, `-p vxn-app`,
      `--workspace` green; vxn-1 web node suite green.
- [ ] Manual DAW pass ([[verify-audio-in-reaper]]) + a browser pass: automate,
      load a preset, undo, flip a sync toggle, reopen the editor.

## Notes

- vxn-1 is released ([[vxn-release-process]]); this is a mechanism swap in a
  shipped product with no audible signature, so the manual passes are the real
  acceptance and the suites are the floor.
- Two model impls means two drain sites ([[0304]]). Converting one and not the
  other leaves vxn-1's web build on a mechanism its native build no longer has —
  worse than either end state.
- One `cargo test` at a time — [[vxn-no-parallel-cargo-test]]. No `cargo fmt` —
  [[vxn-no-cargo-fmt]]. Stage explicit paths —
  [[vxn-concurrent-vxn2-work-no-git-add-all]].
- Closes E046 with [[0303]].

## Close-out (2026-08-27) — won't-do, vxn-1 retired

Closed unbuilt, with [[0304]] which it depended on. vxn-1's CLAP shell and web
controller are archived, and `vxn-app::diff` — "the poll that started the whole
idiom", which this ticket existed to delete — went with them. The deletion
happened, just not the way the ticket planned it.

[E046](../../epics/open/E046-dirty-bitset-pump-vxn1-vxn1b.md) loses its vxn-1
arm entirely and is now the vxn-1b chain alone.
