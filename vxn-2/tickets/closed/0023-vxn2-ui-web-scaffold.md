---
id: "0023"
title: vxn2-ui-web crate scaffold (wry child WebView + IPC bridge)
priority: high
created: 2026-06-06
closed: 2026-06-06
epic: E003
---

## Summary

Stand up the `vxn2-ui-web` crate: bundle the VXN2 faceplate assets and
delegate the WebView lifecycle, IPC bridge, batched view-event sink,
corpus snapshot push, and macOS text-input popup to
`vxn-core-ui-web::open_editor`. The crate's job is to splice the
HTML / CSS / JS / params JSON into one HTML string, build a
`WebEditorConfig` that supplies VXN2's `parse_custom_ui` and
`serialise_custom_view` closures, and re-export the resulting
`EditorHandle` for `vxn2-clap` to mount.

No WebView ownership beyond the config + open call — every wry /
raw-window-handle dependency stays in `vxn-core-ui-web`. This crate
adds approximately one `lib.rs` plus an `assets/` tree.

## Acceptance criteria

- [ ] `vxn-2/crates/vxn2-ui-web` added to the workspace `members` list
      and given a `workspace.dependencies` alias.
- [ ] Dependencies: `vxn-core-app`, `vxn-core-ui-web`, `vxn2-engine`,
      `vxn2-app`, `serde_json`. NO direct `wry` / `raw-window-handle`
      deps — those stay in core.
- [ ] `assets/` tree contains a placeholder `index.html` (full port
      lands in 0025), `style.css`, a `panels/` directory, and a
      `params.json` build artefact slot. Files are pulled in at build
      time via `include_str!` / `include_bytes!`.
- [ ] `pub const EDITOR_WIDTH: u32 = 1024; pub const EDITOR_HEIGHT: u32 = 772;`
      matching the ui-mockup canvas size.
- [ ] `pub fn open_editor(parent, ctrl, corpus) -> EditorHandle`
      composes the HTML, builds a `WebEditorConfig` with VXN2's
      custom hooks (`parse_custom_ui` for mod-matrix / op-tab / edit-
      layer opcodes; `serialise_custom_view` for the matching view
      events), and calls `vxn_core_ui_web::open_editor`. The returned
      handle is re-exported as `vxn2_ui_web::EditorHandle`.
- [ ] `parse_custom_ui` recognises opcodes `set_edit_layer`,
      `set_op_tab`, `set_matrix_row` (and any others added in
      0027 / 0028); returns the matching
      `UiEvent::Custom(Vxn2UiCustom::...)`.
- [ ] `serialise_custom_view` translates each `Vxn2ViewCustom`
      variant to its `{ "kind": "...", ... }` JSON shape.
- [ ] Params JSON helper: a build-time-or-runtime function that walks
      `vxn2_engine::params::PARAMS` and emits a JSON array the page
      hydrates into `vxn.params` on first batch. Reuses
      `vxn_core_ui_web::descriptor_to_json`.
- [ ] Placeholder page loads under wry: a `cargo test
      -p vxn2-ui-web` integration test that mounts the editor against
      a stub controller and confirms the IPC handler is wired (post
      a synthetic `ready` opcode, assert `UiEvent::EditorReady`
      lands on the controller's UI rx). Skips on non-macOS for now.
- [ ] `pub use vxn_core_ui_web::EditorHandle;` and
      `pub use vxn_core_ui_web::prompt_text;` so 0024 / 0030 can
      reach them through this crate.

## Notes

- Mirror `vxn-1/crates/vxn-ui-web` in shape, but it's much smaller now
  that the WebView lifecycle, IPC plumbing, and text-input popup all
  live in `vxn-core-ui-web`. The VXN1 crate is 1963 lines because it
  still owns the panels JS / params JSON builders; the VXN2 crate
  should ship at well under half that (target: <800 lines including
  assets glue, panel JS lands in 0026).
- Pin `wry` and `raw-window-handle` through `vxn-core-ui-web` only —
  do NOT add direct deps here, that would risk version skew if a
  future bump lands in core but not here.
- WebView2 user-data folder: pass `webview2_vendor = Some("Vulpus")`
  and `webview2_product = Some("VXN2")` in the `WebEditorConfig` so
  Windows hosts don't fail with `E_ACCESSDENIED`.
- Don't pre-build the HTML string at module load — splice on
  `open_editor` so a future hot-reload mode (env var → read from
  filesystem) can drop in without changing the call site.
