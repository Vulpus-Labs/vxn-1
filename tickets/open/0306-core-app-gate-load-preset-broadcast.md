---
id: "0306"
product: monorepo
title: "vxn-core-app: gate load_preset's broadcast on echo_param_writes"
priority: medium
created: 2026-08-25
epic: E046
depends: []
---

## Summary

Ticket of [E046](../../epics/open/E046-dirty-bitset-pump-vxn1-vxn1b.md), and a
hard dependency of both shell rewires ([[0302]], [[0305]]) — not a cleanup.

[`load_preset`](../../crates/vxn-core-app/src/controller.rs#L435) calls
`broadcast_all_params()` unconditionally. When a synth also runs a dirty pump,
the model restore flips every bit and the pump re-emits the same table on the
same tick: ~2× the records and a wasted display string per param, per load.
[`HostEvent::StateLoaded`](../../crates/vxn-core-app/src/controller.rs#L400-L405)
was gated on `echo_param_writes` for exactly this reason in ticket 0067;
`load_preset` was missed.

vxn-2 has been paying this since 0067 (finding 1 of
[0298](0298-vxn2-web-controller-smells.md)). It becomes E046's problem the
moment vxn-1 or vxn-1b turns echo off, so it lands here rather than waiting on
0298's verdict.

## Design

Gate it the same way `StateLoaded` is gated, with the same comment. That leaves:

- **echo on** (vxn-1, vxn-1b today, all three before E046): the broadcast is the
  *only* emitter for a preset load and must fire. Unchanged.
- **echo off** (vxn-2 today, all three after E046): the pump re-emits from the
  bits `restore_from_bytes` flipped. Broadcast suppressed.

Watch the second-order effect vxn-2 currently depends on by accident: the
broadcast copy carries `descriptor.display()` while the pump copy carries
`sync_aware_display`, and it renders correctly only because the pump drain runs
*after* the `view_rx` drain and the JS keeps the last record per id (finding 2 of
[[0298]]). Removing the broadcast removes the wrong-display copy entirely — a
fix, but it means vxn-2's rendering stops depending on drain order, so re-check
rather than assume no visible change.

`step_preset` routes through `load_preset`, so it is covered; check for any other
caller before assuming that.

## Acceptance criteria

- [ ] `load_preset`'s `broadcast_all_params()` is gated on `echo_param_writes`,
      commented like `StateLoaded`'s.
- [ ] Test with echo **on**: a preset load emits one `ParamChanged` per param.
- [ ] Test with echo **off**: a preset load emits **zero** `ParamChanged` from
      the controller (the model's bits are the emitter).
- [ ] vxn-2: a preset load's view batch halves; synced rate labels still render
      as subdivisions after a load, verified in the browser, not only in tests.
- [ ] vxn-1 and vxn-1b (still echo-on at this point) show no behaviour change —
      full suites green, and a manual preset load in each
      ([[verify-audio-in-reaper]]).
- [ ] `cargo test -p vxn-core-app` and all three products' suites green.

## Notes

- This is the one `vxn-core-app` edit in E046. All three synths run this code
  path; a mistake here is a mistake in every product at once.
- Overlaps [[0298]] finding 1 deliberately. Whichever lands first, the other
  records it as done rather than re-fixing.
- One `cargo test` at a time — [[vxn-no-parallel-cargo-test]]. No `cargo fmt` —
  [[vxn-no-cargo-fmt]].
- Blocks 0302 and 0305.
