---
id: "0115"
product: vxn-1
title: Host-boundary panic safety (wry WebView build) — no unwind across the C ABI
priority: high
created: 2026-06-10
epic: E011
---

## Summary

The structural review (2026-06-10) flagged one UB-class defect: the
`wry` WebView build in the editor's `open_editor` path could panic, and
that unwind would cross the CLAP C ABI back into the host — undefined
behaviour, host crash. Convert the build failure into a `Result`
surfaced through `set_parent` as a `PluginError`, so a forced WebView
build failure leaves the plugin alive and rendering.

## Acceptance criteria

- [ ] `open_editor` returns a `Result`; the `wry` `.build()` failure is
      mapped to a typed error, not `.expect()`/`.unwrap()`.
- [ ] `set_parent` maps that error to `PluginError` (via `?`), so no
      panic can unwind across the host call.
- [ ] Remaining `expect`/`panic`/`unwrap` paths reachable from a CLAP
      entry point in `vxn-clap`/`vxn-ui-web` are audited — production
      code is panic-free; the survivors are `#[cfg(test)]` only.

## Notes

Scaffolded retroactively (2026-06-22): the work landed during the E020
web-ship refactor of the shared `vxn-core-ui-web::open_editor` and was
never given its own ticket. This file records and verifies the state for
the E011 trail.

## Close-out (2026-06-22)

Verified done in the current tree — the fix shipped with the shared
editor backend refactor (E020).

- Shared backend returns a `Result` and maps the `wry` build failure:
  `vxn_core_ui_web::open_editor` ends `.build().map_err(OpenEditorError::WebViewBuild)?`
  ([lib.rs:419](../../crates/vxn-core-ui-web/src/lib.rs#L419)).
- The vxn-1 wrapper propagates it:
  `vxn_ui_web::open_editor -> Result<EditorHandle, OpenEditorError>`
  ([lib.rs:65](../../vxn-1/crates/vxn-ui-web/src/lib.rs#L65)), doc comment
  states "never panics … the clack shell maps it to `PluginError`".
- The host boundary maps via `?`:
  `set_parent` ends `self.gui = Some(vxn_ui_web::open_editor(...)?)` with
  the comment "Construction failure (bad parent, wry build error)
  surfaces as PluginError … never a panic across the host's C ABI (0115)"
  ([gui.rs:64](../../vxn-1/crates/vxn-clap/src/gui.rs#L64)). The per-OS
  parent accessors also return `PluginError::Message` on a null handle,
  not a panic.
- Panic-path audit: every remaining `expect`/`unwrap`/`panic!` in
  `vxn-ui-web`/`vxn-clap` reachable from a host call is `#[cfg(test)]`.
  The two production `expect`s in `parse_ui_event`/serialisation paths
  operate on data the plugin itself just produced; the line `388`
  serialise path uses `unwrap_or_default()`, not `unwrap()`.
