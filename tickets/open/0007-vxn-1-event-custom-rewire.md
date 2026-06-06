---
id: "0007"
title: vxn-1 event-Custom rewire onto vxn-core-app types
priority: high
created: 2026-06-06
epic: E001
---

## Summary

Route vxn-1's synth-specific `UiEvent` / `ViewEvent` variants through
`vxn_core_app::{UiEvent, ViewEvent}::Custom` payloads so vxn-1's
`Controller`, `EditorBackend`, and the IPC bridge can be unified onto
the shared types. Unblocks 0008 (vxn-clap) and 0009 (vxn-ui-web)
migrations — both currently can't consume the shared crates because
their trait signatures cascade vxn-1's local enum types.

## Acceptance criteria

- [ ] `vxn_app::Vxn1Params` extension trait — `key_mode() -> KeyMode`,
      `set_key_mode(KeyMode)`, `set_key_mode_seeded(KeyMode)`,
      `split_point() -> u8`, `set_split_point(u8)`. Generic over
      `M: vxn_core_app::ParamModel + Vxn1Params`; vxn-engine's
      `SharedParams` impls both.
- [ ] `vxn_app::events::Vxn1UiCustom` enum: `SetKeyMode(KeyMode)`,
      `SetSplitPoint(u8)`, `SetEditLayer(Layer)`, `ResetLayer(Layer)`.
      Boxed payloads ride `UiEvent::Custom`.
- [ ] `vxn_app::events::Vxn1ViewCustom` enum: `KeyModeChanged(KeyMode)`,
      `SplitPointChanged(u8)`, `EditLayerChanged(Layer)`. Boxed
      payloads ride `ViewEvent::Custom`.
- [ ] vxn-1 `UiEvent` / `ViewEvent` re-exports drop the local enums
      and use `vxn_core_app::{UiEvent, ViewEvent}`. `vxn_app::events`
      module shrinks to just the `Vxn1UiCustom` / `Vxn1ViewCustom`
      helpers + `PresetSource` / `HostEvent` re-exports.
- [ ] vxn-1's `Controller` becomes a thin wrapper that constructs
      `vxn_core_app::Controller<M>` and supplies the
      `on_custom_ui` / `on_custom_host` closures that downcast
      `Vxn1UiCustom` payloads and run the original synth-specific
      logic (`set_key_mode_seeded` + broadcast, `snap_to_upper_if_whole`,
      etc.).
- [ ] All 40 call sites across `vxn-app` / `vxn-clap` / `vxn-ui-web`
      that construct or match on the vxn-1-specific event variants
      are rewritten to construct / downcast `Custom` payloads.
- [ ] vxn-1's tests pass unchanged (no test deletions). New tests
      cover the Custom downcast path in the Controller.
- [ ] `vxn_app::EditorBackend` is dropped (vxn-1 uses
      `vxn_core_app::EditorBackend` directly now that the event types
      are shared).
- [ ] Wire-format compat: the JS bridge keeps the same `{op: "set_key_mode", ...}`
      / `{kind: "key_mode_changed", ...}` opcodes — the rewire is
      purely an internal Rust-side reshape. (Verified by the
      vxn-1 web E2E test suite if one runs; otherwise by reading the
      JS bridge code unchanged.)

## Notes

The original `Controller` event loop semantics (gesture brackets,
ordering, broadcast_all_params after key-mode flip) must be preserved
exactly. Lift the old `handle_ui` arm bodies for each Vxn1UiCustom
variant into the on_custom_ui closure verbatim — they're synth-specific
but their *shape* is the same.

`snap_to_upper_if_whole` is private state on the old Controller; the
custom handler can call `vxn_core_app::Controller::push_view_event` +
`broadcast_all_params` directly (both pub).

`vxn-1` vxn-app's `controller.rs` becomes ~150 LOC of thin wrapper
plus the Vxn1Custom handler. The 486-LOC original goes away.
