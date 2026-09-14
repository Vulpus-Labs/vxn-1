---
id: "0370"
product: vxn-3
title: "vxn-3 editor tick on TickSource"
priority: medium
created: 2026-09-10
epic: E051
depends: ["0367", "0368"]
---

## Summary

Same change as [0368](0368-vxn2-tick-source.md), against `vxn3-clap`:
registration at [gui.rs:46](../../vxn-3/crates/vxn3-clap/src/gui.rs#L46),
teardown at [gui.rs:24](../../vxn-3/crates/vxn3-clap/src/gui.rs#L24). Fourth
ticket of [E051](../../epics/open/E051-audio-unit-distribution.md).

## Acceptance criteria

- [ ] `tick: TickSource` replaces the `Option<(HostTimer, TimerId)>` field;
      `on_timer` and a new `on_main_thread` share one `editor_tick`.
- [ ] Behaviour unchanged under a `timer-support` host; the lane editor still
      animates the playhead at the same rate.
- [ ] Existing `vxn3-clap` tests green.

## Notes

vxn-3's tick also drives transport-position readback, so a dropped tick is more
visible here than in vxn-2 — worth eyeballing the playhead specifically rather
than only knob echo.

vxn-4 is deliberately absent from this phase: it has no faceplate yet, so there
is no tick to drive. It picks the pattern up when its editor lands.
