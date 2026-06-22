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

## Close-out (2026-06-22)

- Core `Controller::tick` gains a third arg `on_model_loaded: &mut
  ModelLoadedHook<M>` (a `dyn FnMut(&mut Controller<M>)` typedef,
  symmetric with the new `CustomHandler` typedef for the existing custom
  callbacks). Fires from inside the load paths after the model mutates +
  re-broadcasts: `load_preset`, `step_preset` (via `load_preset`), and
  `HostEvent::StateLoaded`.
  [controller.rs:226](../../crates/vxn-core-app/src/controller.rs#L226),
  [:406](../../crates/vxn-core-app/src/controller.rs#L406),
  [:436](../../crates/vxn-core-app/src/controller.rs#L436).
- vxn-1 emits `KeyModeChanged`/`SplitPointChanged`/`EditLayerChanged` from
  the hook (`publish_keymode_split`) at the moment of load; direct UI edits
  (`SetKeyMode`/`SetSplitPoint`) emit inline from `handle_ui_custom`.
  [controller.rs](../../vxn-1/crates/vxn-app/src/controller.rs).
- Poll-and-diff removed: `last_key_mode`/`last_split_point`/`first_tick`
  fields + `publish_keymode_split_diffs` deleted from the vxn-1 wrapper.
- `editor_ready` force-republish kept — `take_editor_ready_flag()` still
  calls `publish_keymode_split` on (re-)attach (webview reload concern,
  separate from model load).
- vxn-2 (`tick_vxn2`) and vxn-3 (`tick_vxn3`) pass a no-op `on_loaded`
  closure — neither has load-driven non-param view state.
- Tests: existing load/editor-ready coverage now asserts hook-sourced
  emission (`vxn-app/tests/controller.rs`
  `preset_load_emits_per_param_view_events`,
  `editor_ready_replays_params_and_corpus`, the edit-layer-snap tests).
  Added `out_of_band_model_change_does_not_emit_keymode_split` — mutates
  the model behind the controller and asserts a bare tick emits nothing,
  proving the poll is gone. Core test got the no-op hook arg. 17 vxn-app
  controller tests pass; `cargo test --workspace` green (69 suites);
  clippy clean.
