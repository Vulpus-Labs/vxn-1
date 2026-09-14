---
id: "0368"
product: vxn-2
title: "vxn-2 editor tick on TickSource — one tick body, two entry points"
priority: high
created: 2026-09-10
epic: E051
depends: ["0367"]
---

## Summary

Put `vxn2-clap` on 0367's `TickSource` so the faceplate stays live under a host
with no `timer-support`. Second ticket of
[E051](../../epics/open/E051-audio-unit-distribution.md).

Today `VxnMainThread` holds `timer: Option<(HostTimer, TimerId)>`
([lib.rs:173](../../vxn-2/crates/vxn2-clap/src/lib.rs#L173)), registers in
`set_parent` ([gui.rs:55](../../vxn-2/crates/vxn2-clap/src/gui.rs#L55)),
unregisters in `destroy` ([gui.rs:21](../../vxn-2/crates/vxn2-clap/src/gui.rs#L21))
and does the work in `on_timer`
([lib.rs:281](../../vxn-2/crates/vxn2-clap/src/lib.rs#L281)).

## Design

Extract the `on_timer` body verbatim into `fn editor_tick(&mut self)` — pull
intents, `drain_view_events`, `push_model_diffs`, one `flush_view_events`. Both
entry points call it:

```rust
fn on_timer(&mut self, _id: TimerId) { self.editor_tick() }

fn on_main_thread(&mut self) {
    if self.gui.is_none() { return }   // editor gone; let the loop die
    self.editor_tick();
    if let Some(host) = self.host.as_mut() { self.tick.rearm(host) }
}
```

`timer` becomes `tick: TickSource`; `set_parent` assigns `TickSource::start`,
`destroy` calls `stop` **before** dropping the `EditorHandle`, so no tick can
observe a half-torn editor.

The `gui.is_none()` guard is the whole safety story for the fallback arm: a
re-arm after teardown is a callback loop with no owner.

## Acceptance criteria

- [ ] `on_timer` and `on_main_thread` share one `editor_tick`; no duplicated
      drain sequence.
- [ ] `tick: TickSource` replaces the `Option<(HostTimer, TimerId)>` field.
- [ ] Under a host with `timer-support` (Live, Bitwig) the behaviour is
      **unchanged** — the `Host` arm is taken, `request_callback` never fires.
      Verified by opening the editor and driving automation.
- [ ] Existing `vxn2-clap` tests green; `cargo clippy -p vxn2-clap` clean.

## Notes

- Ordering in `destroy` matters and is the one thing worth a comment: stop the
  tick, then close the handle.
- vxn-1b (0369) and vxn-3 (0370) are the same change against their own tick
  bodies; land this one first and copy the shape.
