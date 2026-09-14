---
id: "0369"
product: vxn-1b
title: "vxn-1b editor tick on TickSource"
priority: medium
created: 2026-09-10
epic: E051
depends: ["0367", "0368"]
---

## Summary

Same change as [0368](0368-vxn2-tick-source.md), against `vxn1b-clap`:
registration at [gui.rs:73](../../vxn-1b/crates/vxn1b-clap/src/gui.rs#L73),
teardown at [gui.rs:27](../../vxn-1b/crates/vxn1b-clap/src/gui.rs#L27). Third
ticket of [E051](../../epics/open/E051-audio-unit-distribution.md).

vxn-1b's `destroy` also stops scope capture with the window
([gui.rs:32](../../vxn-1b/crates/vxn1b-clap/src/gui.rs#L32)) — that side effect
stays where it is, and the tick stop goes ahead of it.

## Acceptance criteria

- [ ] `tick: TickSource` replaces the `Option<(HostTimer, TimerId)>` field;
      `on_timer` and a new `on_main_thread` share one `editor_tick`.
- [ ] Scope capture still stops on editor close, in the same order relative to
      handle teardown as today.
- [ ] Behaviour unchanged under a `timer-support` host; existing tests green.

## Notes

vxn-1b takes the period from a local `period` binding rather than
`WEBVIEW_TIMER_PERIOD_MS` ([gui.rs:73](../../vxn-1b/crates/vxn1b-clap/src/gui.rs#L73));
fold it onto the core constant while here unless it is deliberately different,
in which case say so in a comment.
