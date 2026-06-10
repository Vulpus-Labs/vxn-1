---
id: "0065"
title: "Emit CLAP param_gesture_begin/end from the audio thread"
priority: high
created: 2026-06-10
epic: E006
depends: []
---

## Summary

Fifth ticket of [E006](../../epics/open/E006-review-remediation.md).
The review found zero occurrences of
`ParamGestureBeginEvent` / `ParamGestureEndEvent` in `vxn2-clap`. The
controller populates `SharedParams.gestures` on UI
`BeginGesture` / `EndGesture` (via `vxn-core-app`), and ADR 0003
§"What survives" says the gesture bitset "drives CLAP
`gesture_begin`/`gesture_end` events out to the host" — but
`LocalParams::emit`
([local.rs](../../crates/vxn2-clap/src/local.rs)) only emits
`ParamValueEvent`. Without brackets, conformant hosts
(Bitwig, Reaper) record knob-drag automation incorrectly or refuse the
gesture entirely.

VXN-1 already solved this: `vxn-core-clap/src/gesture.rs` plus the
emit-side consumption in its `local.rs`. Port that pattern.

## Where

- `local.rs::emit` — track per-param gesture state on the audio-thread
  mirror; when the shared gesture bit transitions 0→1, push
  `ParamGestureBeginEvent` before the first `ParamValueEvent` for that
  id; on 1→0, push `ParamGestureEndEvent` after the last. The unused
  `_shared: &SharedParams` / `_frame_count: u32` parameters on `emit`
  ([local.rs:127-128](../../crates/vxn2-clap/src/local.rs#L127)) were
  reserved for exactly this — use them or drop them.
- Reading `SharedParams.gestures` from the audio thread must stay
  lock-free atomic loads — same discipline as the value table.
- Check the VXN-1 implementation for edge cases it already handles:
  gesture begin with no value change, editor closed mid-gesture,
  state-load during gesture.

## Acceptance criteria

- [ ] Drag bracket emits, in order, on the host's output event queue:
  `gesture_begin(id)` → `value(id)`× N → `gesture_end(id)`. Asserted
  by a clack-host smoke test (extend
  [smoke.rs](../../crates/vxn2-clap/tests/smoke.rs)) that injects
  `BeginGesture` / `SetParam` / `EndGesture` UI intents and inspects
  the output events of the next `process()` calls.
- [ ] No gesture events emitted for host-driven automation (host's own
  `ParamValueEvent` input must not echo back wrapped in brackets).
- [ ] No allocation introduced in `emit` (it runs in `process()`).
- [ ] Manual test in Reaper or Bitwig: write-arm a track, drag a knob
  in the editor, confirm a single clean automation gesture is
  recorded.

## Notes

Most pressing host-facing gap from the review. Land before 0067 (echo
removal) so gesture behaviour is testable while the view event flow is
otherwise stable.

## Close-out (2026-06-10)

VXN-1 `vxn-clap` pattern ported into `LocalParams::emit`: per-param
last-seen gesture bools on the audio-thread mirror, 0→1 edge pushes
`ParamGestureBeginEvent` before the value, 1→0 pushes
`ParamGestureEndEvent` after it (at `frame_count − 1`), and a bare
value change with no surrounding gesture wraps itself in its own
begin/end. The previously reserved `_shared` / `_frame_count` params
are now used. Gesture reads are lock-free atomic loads; no allocation.

Deviations / notes:

- Tests live in `local.rs` (unit level) rather than the clack-host
  smoke harness: the smoke host has no UI-intent path — gesture state
  is injected via `SharedParams::set_gesture`, the same surface the
  controller drives, matching the existing lib.rs test precedent.
  Order, bare-wrap, host-echo-silence and bracket-without-value all
  asserted.
- Found upstream: the pinned clack rev's `CoreEventSpace::from_unknown`
  is missing the two gesture TYPE_ID match arms, so `as_core_event()`
  never yields the gesture variants. Wire events are spec-correct;
  tests decode via typed `as_event::<E>()`. Worth an upstream report.
- Host-driven automation cannot echo back bracketed by construction:
  `apply_input` touches neither `ui_changed` nor the gesture bitset.
- Manual Reaper/Bitwig write-arm check still pending (manual AC).
