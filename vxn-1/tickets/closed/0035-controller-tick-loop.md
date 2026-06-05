---
id: "0035"
title: Controller tick loop — drain UI + host queues, emit view events
priority: high
created: 2026-05-30
epic: E009
---

## Summary

Flesh out `Controller<M: ParamModel>::tick(&mut self)` to drain both
inbound mpsc queues and apply each event's effect to the model. For
this ticket the controller is **still not wired** to either a real UI
or the host — the focus is the loop itself and its unit-testable
semantics. After this lands, every `UiEvent` and `HostEvent` variant
has a defined effect on the model and a defined set of `ViewEvent`s
it emits.

## Acceptance criteria

- [ ] `Controller::tick` drains UI then host queues (UI first so an
      ongoing gesture is bracketed correctly when a host automation
      arrives in the same tick).
- [ ] Each `UiEvent` variant has its handler:
      - `SetParam` / `SetParamNorm` → write model + emit
        `ParamChanged`.
      - `BeginGesture` / `EndGesture` → write model's gesture flag.
      - `LoadPreset` → load (factory or user path), bulk-apply to
        model, emit `PresetLoaded` and one `ParamChanged` per param.
      - `SavePreset` → snapshot model + `save_performance_in`, emit
        `Status`.
      - `RenamePreset` / `DeletePreset` → IO + `PresetCorpusChanged`.
      - `SetKeyMode` / `SetSplitPoint` / `SetEditLayer` → opaque
        state + `KeyModeChanged` / `Status`.
- [ ] Each `HostEvent` variant has its handler:
      - `ParamAutomation` → write model + emit `ParamChanged`.
      - `StateLoaded` → bulk-apply + `PresetLoaded` (empty meta) +
        one `ParamChanged` per param + `KeyModeChanged`.
      - `Tempo` → not stored in model; carried through to engine via
        a separate channel later (out of scope here — just a stub).
- [ ] Unit tests in `vxn-app/tests/`:
      - `ui_set_param_emits_view_event` — `SetParam` round-trips
        through a `MockModel`.
      - `host_automation_echo_suppressed_during_gesture` — UI gesture
        in flight, host event arrives; model updated, but no
        ViewEvent emitted for that param until gesture ends. (Echo
        guard belongs to the controller.)
      - `preset_load_emits_per_param_view_events` — bulk
        `ParamChanged` set after a `LoadPreset`.
- [ ] `cargo test --workspace` passes.

## Notes

`MockModel` is a `HashMap<ParamId, f32>` + a `HashMap<ParamId, bool>`
for gestures, implementing `ParamModel` for tests. Lives in
`vxn-app/tests/common.rs`.

The "echo suppression during gesture" rule is the controller-level
analogue of vxn-clap's current `LocalParams` echo guard, lifted from
the audio thread context into the main thread. This ticket *defines*
the rule; 0036 makes the clack shell actually post the host events
that exercise it.

No vizia or wry touched yet. Controller has no editor reference.
