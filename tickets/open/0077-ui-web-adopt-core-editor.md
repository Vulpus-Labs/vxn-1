---
id: "0077"
product: vxn-1
title: vxn-ui-web — adopt the shared vxn-core-ui-web editor, delete the fork
priority: high
created: 2026-06-21
epic: E024
---

## Summary

`vxn-ui-web/src/lib.rs` carries its own near-verbatim copy
of the entire WebView editor infrastructure that already
lives in `vxn-core-ui-web` — which `vxn-ui-web` already
depends on. Forked surface: `EditorHandle` (struct +
`push_view_event`/`flush_view_events`/`open_text_input`/
`serialize_corpus`/`close`), the `WebEditor`
`EditorBackend` impl, `open_editor`, `ParentWindow` +
its `unsafe Send/Sync`/`HasWindowHandle`, all three
`build_raw` cfg arms, and `ensure_webview2_data_dir`
(`vxn-ui-web/src/lib.rs:46-264, 479-531`). The matching
shared code is `vxn-core-ui-web/src/lib.rs:170-455`.

This is dead-weight duplication, not necessary divergence:
**vxn-2's `vxn2-ui-web` already does it correctly** — a thin
shim that builds a `WebEditorConfig` and calls
`vxn_core_ui_web::open_editor`, re-exporting `EditorHandle`/
`OpenEditorError` (`vxn-2/crates/vxn2-ui-web/src/lib.rs:
21-70`). The shared `WebEditorConfig` exists precisely to
carry the per-synth bits vxn-1 hand-rolls. vxn-1 predates
the extraction and never migrated, so the corpus-seeding
flush, the 100KB batch-chunking, the unsafe parent-handle
plumbing, and the WebView2 `set_var` soundness handling are
now forked and must be kept in lockstep by hand.

Migrating also discharges E011's "wry build panic →
`Result`" scope item: the shared `open_editor` already
returns `Result`, so vxn-1 inherits the non-unwinding path
for free.

## Acceptance criteria

- [ ] `vxn-ui-web::open_editor` is implemented by building a
      `vxn_core_ui_web::WebEditorConfig` (HTML, dimensions,
      `uncategorised_label = UNCATEGORIZED`,
      `parse_custom_ui = PARSE_CUSTOM`,
      `serialise_custom_view = SERIALISE_CUSTOM`, WebView2
      vendor/product) and calling
      `vxn_core_ui_web::open_editor`, matching the vxn-2
      pattern.
- [ ] `EditorHandle`/`WebEditor`/`open_editor`'s inner
      body/`ParentWindow`/`build_raw`/
      `ensure_webview2_data_dir` are deleted from
      `vxn-ui-web`; `EditorHandle`/`OpenEditorError` are
      re-exported from `vxn-core-ui-web`.
- [ ] The public signature `vxn_ui_web::open_editor` (called
      from `vxn-clap/src/gui.rs:96`) is preserved, or
      gui.rs updated in the same change; the clap shell
      still builds.
- [ ] A forced WebView build failure (e.g. bad parent
      handle) returns an error rather than panicking; the
      plugin stays alive and audio keeps rendering. This
      closes E011's wry-panic item — note that in 0018's
      epic-state sweep / E011.
- [ ] Any vxn-1-specific behaviour the fork added that the
      shared path lacks is moved into `WebEditorConfig`/the
      shared crate, not left behind as a reason to keep the
      fork. If a genuine vxn-1 delta cannot be expressed via
      config, it stays as a thin documented shim — silent
      parallel copies do not pass.
- [ ] The HTML-structure substring/tally tests in
      `vxn-ui-web/src/lib.rs` (`faceplate_row_panel_widths_
      match_vizia`, `control_tallies_match_all_rows`, …) are
      retained or relocated so faceplate assembly stays
      covered after the code moves.
- [ ] `cargo test --workspace` green; `cargo test
      -p vxn-ui-web` green with `VXN_JS_TESTS=1`; faceplate
      opens and renders unchanged in a host (manual).

## Notes

vxn-2 is the working template — read
`vxn-2/crates/vxn2-ui-web/src/lib.rs` first. Coordinate the
HTML-splice cleanup (0084) with this move: if the splice
logic migrates into the shared crate, do 0084's placeholder
rework there; if it stays vxn-1-side, 0084 lands after this.

This is the single highest-leverage item in E024 — it
removes the most duplicated, hardest-to-keep-synced code in
one move and is low-risk because vxn-2 proves the shape.
