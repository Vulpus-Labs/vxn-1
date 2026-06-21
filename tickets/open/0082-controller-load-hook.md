---
id: "0082"
product: vxn-1
title: core/wrapper — on_model_loaded hook, drop controller poll-and-diff
priority: medium
created: 2026-06-21
epic: E024
---

## Summary

The vxn-1 `Controller` wraps the core `Controller`, but the
core controller's preset-load / state-load paths mutate
key-mode and split-point directly on the model **without**
emitting the vxn-1-specific `Vxn1ViewCustom` events. The
wrapper compensates by snapshotting `last_key_mode`/`last_
split_point` and polling the model after every tick to
detect the change the core made silently
(`vxn-app/src/controller.rs:88-133`, plus the `first_tick`/
`editor_ready` rearm dance at `:99-102`).

This is a leaky abstraction across the core/wrapper
boundary: the core controller is meant to be the sole
mutator-and-emitter, but it half-owns vxn-1 shared state
(mutates it on load) while not knowing how to announce it,
forcing a poll-and-diff shim. The `force`/`first_tick`/
`take_editor_ready_flag` interplay guards a specific
webview-reload bug (comments at `:94-102`), so it is
load-bearing and fragile.

## Acceptance criteria

- [ ] The core `Controller` gains a post-mutation hook for
      preset/state load — e.g. an `on_model_loaded: &mut dyn
      FnMut(&mut Controller<M>)` closure passed to `tick`,
      symmetric with the existing `on_custom_ui`/`on_custom
      _host` callbacks — invoked from inside the load path
      after the model is mutated.
- [ ] vxn-1 emits its `KeyModeChanged`/`SplitPointChanged`
      (`Vxn1ViewCustom`) events from that hook, at the
      moment of mutation.
- [ ] The steady-state poll-and-diff in `vxn-app/src/
      controller.rs` (the `last_key_mode`/`last_split_point`
      snapshot + per-tick comparison) is removed.
- [ ] The `editor_ready` force-republish stays (it is a
      genuinely separate concern — webview reload), and the
      existing controller tests covering it
      (`tests/controller.rs:511-569, 588-664, 1004-1023`)
      still pass; add/adjust a test asserting key-mode/split
      events now fire from the load path, not a later tick.
- [ ] `cargo test --workspace` green.

## Notes

Touches `vxn-core-app` (the hook) and `vxn-app` (the
emitter + poll removal). Keep the hook generic so vxn-2 can
attach its own post-load notifications without re-deriving
the poll pattern.
