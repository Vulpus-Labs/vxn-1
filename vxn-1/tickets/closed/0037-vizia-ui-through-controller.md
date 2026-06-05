---
id: "0037"
title: Route Vizia editor through the controller (UiEvents in, ViewEvents out)
priority: high
created: 2026-05-30
epic: E009
---

## Summary

Replace every direct `shared.set` / `shared.set_gesture` /
`shared.load_performance` / `shared.set_key_mode` etc. call in the Vizia
editor with a `UiEvent` post. Replace the existing `PollAutomation`
re-read of `SharedParams` with a `ViewEvent` drain populated by the
controller.

## Acceptance criteria

- [ ] Every Vizia control callback posts a `UiEvent`; none touch
      `SharedParams` directly. (`grep` for `shared.set\|shared.gesture`
      in `vxn-ui/src/lib.rs` returns nothing.)
- [ ] The editor receives an `mpsc::Receiver<ViewEvent>` at open time;
      its `on_idle` callback drains it and updates the reactive
      signals.
- [ ] `UiModel::event(&PollAutomation, …)` collapses to "drain
      ViewEvents". The bound-control list (`Vec<Ctl>`) still keys the
      signals — `ParamChanged { id, .. }` looks up the right `Ctl` and
      writes its signal.
- [ ] No regressions in editor behaviour:
      - Dragging a fader changes the param + records DAW automation.
      - Host automation playback moves the fader.
      - Loading a preset repaints every control.
      - Key-mode + split-point toggles work.
      - Browser open/load/save behaves as before.
- [ ] `vxn-ui` no longer depends on `vxn-engine`'s preset IO
      (`load_preset_file`, `save_performance_in`, `list_user_tree`
      etc.) — those calls move into the controller. The view receives
      `PresetLoaded` and `PresetCorpusChanged` ViewEvents instead.
- [ ] `cargo test --workspace` passes.

## Notes

The two non-automatable state mirrors in `UiModel` (`key_mode`, `split`)
follow `KeyModeChanged` ViewEvents now, not direct `shared.key_mode()`
reads. Same for the preset name and status strings in `preset_bar`.

The browser's two-pane view becomes a pure projection of the corpus
the controller publishes. The current
`build_browser` / `reseed_browser` machinery moves into the controller
(it's all engine-side IO + filtering); the view binds to a
`Arc<Vec<BrowserEntry>>` signal that the controller's tick refreshes.

This is the biggest ticket in E009 by line count. Land it as one PR
*with the regression test suite green* rather than splitting — the
intermediate state ("half the editor goes through events") is harder
to reason about than the cutover.

After this lands, `vxn-ui` knows only about `vxn-app`. No `vxn-engine`
in its dependency list. (Param descriptors come through ParamModel.)
