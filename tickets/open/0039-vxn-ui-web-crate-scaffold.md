---
id: "0039"
title: Scaffold vxn-ui-web crate (wry + IPC bridge wired to controller)
priority: high
created: 2026-05-30
epic: E010
---

## Summary

Create the `vxn-ui-web` workspace crate: a `wry`-backed WebView editor
that embeds in the host's parent window as a child surface and exposes
an `impl EditorBackend` for vxn-app's `Controller`. The HTML payload
is a placeholder for now (a single div with the plugin name); UiEvent
/ ViewEvent plumbing is the real deliverable. The faceplate content
arrives in 0040–0045.

## Acceptance criteria

- [ ] `crates/vxn-ui-web/` added; workspace member + dependency entry.
- [ ] `EditorHandle`, `open_editor(parent, controller_handle) ->
      EditorHandle` matching the `EditorBackend` trait surface from
      vxn-app.
- [ ] macOS: wry `WebViewBuilder::new_as_child` parented to the
      host NSView, sized to `EDITOR_WIDTH × EDITOR_HEIGHT`.
- [ ] Windows + Linux: cfg-gated `RawWindowHandle` construction
      (Win32, Xcb). Compile-only acceptance: `cargo check --target
      x86_64-pc-windows-msvc` passes. (Runtime parity is a later
      ticket.)
- [ ] IPC bridge: JS posts JSON via `window.ipc.postMessage`; Rust IPC
      handler parses and forwards to `controller.ui_sender()` as the
      matching `UiEvent`.
- [ ] Rust → JS push: `EditorBackend::push_view_event(handle, event)`
      maps the ViewEvent to a JS call via `webview.evaluate_script`.
      For 0039 the payload is just `console.log(eventJson)` — the
      structured DOM updates land per-panel in 0041+.
- [ ] Editor close: dropping `EditorHandle` removes the WebView from
      the parent's subviews.
- [ ] `cargo test -p vxn-ui-web` passes (no UI tests yet; smoke
      ICompile only).

## Notes

The prototype run (now deleted) used `wry = "0.45"`,
`raw-window-handle = "0.6"`, and a single placeholder HTML. That stack
worked end-to-end; reuse the same versions.

IPC handler signature: `Fn(Request<String>) + 'static` (wry 0.45). The
controller's `Sender<UiEvent>` is `Send + Sync`; clone into the
closure.

Bounds + scale: re-use the existing `parent_backing_scale` trick from
`vxn-clap/src/gui.rs` to pin the WebView's logical sizing against
the host's NSView backing scale. (Vizia had the same problem; the same
fix applies.)

The placeholder div is intentionally boring. Land the bridge cleanly
and let 0040 take ownership of the visual content.
