---
id: "0119"
title: LocalParams — lift gesture brackets into vxn-core-clap
priority: medium
created: 2026-06-10
epic: E021
---

## Summary

Three `LocalParams` implementations exist: the generic
`vxn-core-clap/src/local.rs` (`LocalParams<const N>`,
exported and tested, used by **neither** synth in
production), vxn-1's fork (`vxn-clap/src/local.rs`, 201
lines, adds the per-param gesture array + bracket emission)
and vxn-2's fork (write-through `apply_input`, matrix
mirror). The `fetch_ui_changes` diff loop is verbatim in all
three. This is the clearest incomplete-extraction artifact
in the workspace: a dead generic plus two drifting copies.

Resolve it for vxn-1: extend the generic with gesture
bracket emission (the only real vxn-1 delta) and make
`vxn-clap` consume it — or, if that proves strained,
document both the core type and the fork as deliberately
divergent and delete whatever is genuinely dead.

## Acceptance criteria

- [ ] `vxn-core-clap::LocalParams<N>` gains gesture-bracket
      emission: per-param gesture state array, live gesture
      flag read through the `SharedStore` trait (add
      `gesture(i)` to the trait), begin/end events wrapped
      around value events including the "bare" transient
      case — behaviour-identical to the current
      `vxn-clap/src/local.rs:111` logic.
- [ ] `vxn-clap` uses the shared type; the local
      `local.rs` fork is deleted (or reduced to a thin
      newtype if `write_to`-style engine wiring needs a
      home).
- [ ] Existing gesture-bracket unit tests move/port to the
      core crate; vxn-1 behaviour pinned by a test that the
      emitted event sequence (begin, value(s), end) is
      unchanged for: sustained drag, bare transient set,
      host automation during gesture.
- [ ] If unification is abandoned for cause: a comment block
      atop both `vxn-core-clap/src/local.rs` and
      `vxn-clap/src/local.rs` stating the fork is permanent
      and why, and the unused exported surface of the core
      version is trimmed. (Either outcome closes the ticket;
      silent parallel copies do not.)
- [ ] No behaviour change at the host: automation recording
      in a DAW still produces correctly bracketed gestures
      (manual check, same as vxn-2 0065's acceptance).
- [ ] `cargo test --workspace` green.

## Notes

Coordinate with vxn-2 ticket 0065, which ports the vxn-1
gesture pattern to vxn-2 — that work wants to land on the
shared generic, not create a third bracket implementation.
Whichever ticket lands first shapes the shared type; the
second consumes it. vxn-2's write-through `apply_input` and
matrix mirror stay synth-side; they are genuine extensions,
not part of this unification.

`batch_range` deduplication (vxn-2's verbatim copy of
`vxn_core_clap::events::batch_range`) is vxn-2 E006 0071/
0072 territory — not this ticket.
