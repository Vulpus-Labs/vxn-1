---
id: "0032"
title: Bundle assets into vxn2.clap + end-to-end editor smoke test
priority: medium
created: 2026-06-06
epic: E003
---

## Summary

Update the `vxn-2/xtask` `bundle` command to copy the
`vxn2-ui-web/assets/` tree into `vxn2.clap/Contents/Resources/`,
so the editor can find its HTML / CSS / JS from the bundle path
at runtime (mirrors VXN1's bundling). Add an in-process editor
smoke test that mounts the WebView under a stub parent, drives
synthetic IPC messages, and asserts the round-trip lands in
`SharedParams`.

The assets are ALSO embedded via `include_bytes!` /
`include_str!` so the cdylib is self-contained. The bundle copy
exists for developer iteration (edit a CSS file, re-bundle, no
recompile of the cdylib).

## Acceptance criteria

- [ ] `vxn-2/xtask/src/main.rs` `bundle` subcommand copies the
      `vxn-2/crates/vxn2-ui-web/assets/` directory tree into
      `target/{profile}/vxn2.clap/Contents/Resources/`. Path
      shape matches macOS bundle conventions.
- [ ] If the env var `VXN2_DEV_ASSETS=1` is set at runtime,
      `vxn2-ui-web::open_editor` reads HTML / CSS / JS from
      `Contents/Resources/` instead of the `include_bytes!`
      embed. Otherwise — the default — runs from the embed
      (production behaviour, no fs dependency).
- [ ] `cargo xtask bundle && cargo xtask install` produces a
      runnable bundle; opening it in Bitwig surfaces the
      editor without any "missing asset" console errors.
- [ ] Editor smoke test in `vxn2-clap/tests/editor_smoke.rs`:
      - Spawn a stub `wry` parent (offscreen window via wry's
        `WebViewBuilder::new_as_child` against a hidden
        `EventLoop`-driven NSWindow on macOS, gated to macOS
        only; Windows / Linux variants land in a follow-up).
      - Instantiate `vxn2-clap`'s plugin entry in-process,
        call `gui::create` + `gui::set_parent` against the
        stub parent.
      - Post a synthetic IPC message
        `{"op":"set_param","id":<algo_clap_id>,"plain":12.0}`
        from JS via `webview.evaluate_script(
        "window.__vxn.bridge.send(...)")`.
      - Drive the timer tick manually (`on_timer` called from
        the test).
      - Assert `SharedParams::get(algo_clap_id) == 12.0`.
      - Tear down via `gui::destroy`; assert no leaks (the
        editor handle is dropped).
- [ ] `cargo test -p vxn2-clap --test editor_smoke` passes on
      macOS. CI marker: `#[cfg(target_os = "macos")]`
      gate skips elsewhere.
- [ ] Bundle install command writes to
      `~/Library/Audio/Plug-Ins/CLAP/vxn2.clap` — matches the
      0019 install target from E002.

## Notes

- The `include_bytes!` embed remains the production path.
  Hot-reload via `VXN2_DEV_ASSETS=1` exists for developer
  iteration only; production users see the embedded bytes
  unconditionally.
- VXN1's xtask is the structural template — copy the bundle-
  copy logic, plist generation, install path. No new
  infrastructure.
- The smoke test is `macos`-only by default because creating a
  hidden NSWindow is the cheapest offscreen parent we have. A
  Windows variant (`CreateWindowExW` with `WS_DISABLED`) is
  straightforward but defer until a Windows-shipping
  motivation appears.
- The smoke test exercises ONE path: JS → IPC → controller →
  ParamModel. It does NOT exercise audio-thread automation
  → page (0031's diff pump). 0031's verification is via
  manual host playback test; an automated test would require
  a clack-host integration which 0020 already establishes —
  consider extending 0020 instead if a regression motivates.
- This is the gate that closes E003: if the smoke test passes
  and a host shows the editor with the default patch reflecting
  on first open, the epic is done.
