---
id: "0036"
title: Route clack host events through the controller
priority: high
created: 2026-05-30
epic: E009
---

## Summary

In `vxn-clap`, extract every host-originated event currently handled
inline (param automation via `local::LocalParams::apply_input`, state
save/restore via `PluginStateImpl`) into `HostEvent`s posted to the
controller. The audio thread keeps reading atomics; the main thread
delegates to the controller for everything else.

## Acceptance criteria

- [ ] `VxnMainThread` owns a `Controller<SharedParams>` (held behind
      `Arc<Mutex<…>>` if needed for thread safety with audio
      processor's `flush`).
- [ ] `PluginMainThreadParams::flush` posts `ParamAutomation` for each
      input event, then calls `controller.tick()`.
- [ ] `PluginAudioProcessorParams::flush` — audio thread — still does
      its existing `apply_input` against `LocalParams` (audio-path
      latency, ADR 0001), but **does not** drive the controller. The
      main thread's flush picks up the same events; the audio-thread
      flush is for engine latency.
- [ ] `PluginStateImpl::load` posts a `StateLoaded` event (carrying
      the parsed `PluginState`); does not directly call
      `shared.params.restore_from` — the controller does that on tick.
- [ ] `PluginStateImpl::save` reads from the model snapshot the
      controller publishes; behaviour identical to today.
- [ ] All existing vxn-clap integration tests pass.

## Notes

The audio thread's `process` loop is the one place that *must not*
acquire a mutex. Keep its `LocalParams` mirror untouched; the
main-thread controller and the audio-thread mirror both observe
`SharedParams` atomics for shared state. The controller's job is
arbitrating *intent* across UI and host on the main thread; it does
not sit in the audio path.

If `Controller<SharedParams>` does need locking on the main thread
(it likely does — `tick` mutates `self`, and the GUI extension and
`flush` can both call into it), prefer `parking_lot::Mutex` over
`std::sync::Mutex` to avoid poisoning on panic. (Plugins must
unwind, not abort — Cargo.toml `panic = "unwind"`.)

UI is still writing direct to `SharedParams` after this ticket; 0037
fixes that.
