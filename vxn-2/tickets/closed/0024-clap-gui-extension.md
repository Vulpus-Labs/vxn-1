---
id: "0024"
title: CLAP gui + timer extensions, editor mount / teardown
priority: high
created: 2026-06-06
closed: 2026-06-06
epic: E003
---

## Summary

Add the CLAP `gui` and `timer` extensions to `vxn2-clap`: register
both in `declare_extensions`, mount the `vxn2-ui-web` editor on
`gui::set_parent`, register a ~60 Hz main-thread timer on the host's
`HostTimer`, and tear everything down on `gui::destroy` and on plugin
drop. After this ticket the editor opens in a host and runs the
controller tick + view-event flush, but shows the 0023 placeholder
page until 0025.

`vxn2-clap`'s `MainThread` grows the same fields VXN1's does today:
`controller: Arc<Mutex<Controller<SharedParams>>>`, `view_rx`, `corpus`,
`gui: Option<EditorHandle>`, `timer: Option<(HostTimer, TimerId)>`,
`last_seen: Vec<f32>` for the audio-thread diff pump (filled in 0031).

## Acceptance criteria

- [ ] `vxn2-clap/Cargo.toml` adds the `gui` and `timer` features to
      `clack-extensions`, plus deps on `vxn2-app`, `vxn2-ui-web`,
      `vxn-core-app`.
- [ ] `VxnPlugin::declare_extensions` registers `PluginGui` and
      `PluginTimer` alongside the existing `PluginAudioPorts` /
      `PluginNotePorts` / `PluginParams` / `PluginState`.
- [ ] `VxnMainThread` gains:
      - `controller: Arc<Mutex<Controller<SharedParams>>>` constructed
        in `new_main_thread` via `Controller::new(shared.params.clone(),
        Box::new(NoopPresetStore))` (factory bank lands in a later
        epic; 0029 stubs Save/SaveAs/Browse).
      - `view_rx: Arc<Mutex<Receiver<ViewEvent>>>`.
      - `corpus: CorpusHandle`.
      - `gui: Option<vxn2_ui_web::EditorHandle>`, `timer: Option<(HostTimer, TimerId)>`,
        `last_seen: Vec<f32>` (Nan-seeded, sized `TOTAL_PARAMS`).
- [ ] `impl PluginGuiImpl for VxnMainThread`:
      - `is_api_supported` / `get_preferred_api` follow VXN1's pattern
        (default platform API, non-floating only).
      - `create` validates the GUI config; allocates nothing yet.
      - `set_parent` extracts the per-OS parent (`as_cocoa_nsview` /
        `as_win32_hwnd` / `as_x11_handle`), calls
        `vxn2_ui_web::open_editor(parent, ctrl_handle, corpus)`, then
        registers a 16 ms `HostTimer` if available. Failure to get
        the timer is non-fatal (degraded mode).
      - `get_size` returns `GuiSize { width: vxn2_ui_web::EDITOR_WIDTH,
        height: vxn2_ui_web::EDITOR_HEIGHT }`.
      - `destroy` unregisters the timer (best-effort), drops the
        `EditorHandle`.
      - `set_scale` / `show` / `hide` / `set_size` / `set_transient`:
        no-ops returning `Ok(())`.
- [ ] `impl PluginTimerImpl for VxnMainThread::on_timer`:
      - `controller.tick_vxn2()` (the 0022 helper).
      - `drain_view_events` from the view-rx into the editor handle.
      - `push_param_diffs` against `last_seen` (the 0031 path; this
        ticket lands the call site, 0031 wires the body).
      - `handle.flush_view_events()` — single `evaluate_script` per
        tick.
- [ ] Bitwig manual smoke (or another local CLAP host): bundle ships
      with `cargo xtask bundle && cargo xtask install`, host opens
      the editor, shows the placeholder page, timer ticks without
      panic, close + reopen works without leaking the WebView.
- [ ] No `unwrap`/`expect` in the GUI path that the host can hit; a
      lock-poisoned mutex degrades to the inner value via the same
      `lock_mut` helper VXN1 uses.

## Notes

- Mirror `vxn-1/crates/vxn-clap/src/{lib.rs,gui.rs}` exactly for the
  GUI extension lifecycle. Diff lines from VXN1's gui.rs are minimal:
  swap `vxn_ui_web` ↔ `vxn2_ui_web`, swap `vxn_app::Controller` ↔
  the core-app `Controller<SharedParams>`, and call `tick_vxn2`
  instead of `tick`.
- `corpus` is constructed from a no-op `PresetStore` for now (returns
  empty factory + empty user tree). 0029 swaps in the real stub that
  fires the Save / SaveAs / Browse `UiEvent`s as no-ops.
- The 16 ms timer period matches CLAP's 30 Hz minimum-supported
  guarantee with headroom. Hosts that clamp it to 30 ms still feel
  responsive; the floor is the host's, not ours.
- `last_seen` is `vec![f32::NAN; TOTAL_PARAMS]` so the first tick
  after open broadcasts the entire param table to seed the page.
  This is the cheap alternative to a "send everything on
  EditorReady" handshake — the controller already does that path on
  `UiEvent::EditorReady`; `last_seen` covers the audio-thread side.
- Do NOT touch the audio-thread path in this ticket — the `process`
  loop already exists from E002 and stays untouched. 0031 closes the
  `LocalParams::publish` → `SharedParams` → diff loop.
