---
id: "0009"
title: vxn-1 vxn-ui-web migrate onto vxn-core-ui-web
priority: medium
created: 2026-06-06
epic: E001
---

## Summary

Strip vxn-1's `vxn-ui-web` down to an HTML/CSS/JS asset bundler.
Lifecycle (wry WebView open / close), the JS↔Rust IPC bridge, the
batched `evaluate_script` view-event sink, the corpus snapshot JSON,
and the native text-input popup all come from `vxn-core-ui-web` via
`open_editor(parent, ctrl, corpus, config)`.

## Prerequisites

- 0007 closed: vxn-1's event types are now `vxn_core_app::{UiEvent,
  ViewEvent}`. The IPC bridge's parse_ui_event_default recognises
  the shared opcodes; vxn-1-specific opcodes (`set_key_mode`,
  `set_split_point`, `set_edit_layer`, `reset_layer`) route through
  the synth's `parse_custom_ui` closure into
  `UiEvent::Custom(Box<Vxn1UiCustom>)`. Similarly for
  `serialise_custom_view`.

## Acceptance criteria

- [ ] vxn-1 `vxn-ui-web` is reduced to:
      - `splice_html()` — combine vxn-1's CSS + 4 JS modules +
        params JSON + subdivisions JSON + base HTML into a single
        `String`. Vxn-1-asset specific; not in vxn-core-ui-web.
      - `parse_vxn1_custom_ui(op, &json) -> Option<UiEvent>` — recognises
        `set_key_mode` / `set_split_point` / `set_edit_layer` /
        `reset_layer` and returns the matching `UiEvent::Custom`.
      - `serialise_vxn1_custom_view(&dyn Any) -> Option<serde_json::Value>` —
        downcasts `Vxn1ViewCustom` and emits the page's JSON shape.
      - A thin `open_editor()` re-export or shim that calls
        `vxn_core_ui_web::open_editor` with the right `WebEditorConfig`
        (vendor = "VulpusLabs", product = "VXN1", uncategorised_label,
        + the parse/serialise closures above).
- [ ] vxn-1 `vxn-ui-web/src/text_input.rs` deleted.
      `vxn_core_ui_web::prompt_text` covers macOS + Windows.
- [ ] vxn-1 `vxn-ui-web/src/lib.rs` `WebEditor` struct deleted.
      vxn-1 vxn-clap calls `vxn_core_ui_web::open_editor` directly.
- [ ] vxn-1 `vxn-ui-web/src/lib.rs` `batch_chunks` /
      `dedup_param_changes` / `view_event_to_json` / `parse_ui_event`
      / `corpus_snapshot_json` deleted.
- [ ] vxn-1 tests pass. The faceplate page still loads in a host
      (manual: open vxn-1.clap in Bitwig / clack-host, turn a knob,
      observe sound + view echo). The JS bridge's wire format
      (opcodes, kinds) is unchanged.
- [ ] The Windows WebView2 user-data dir override (planted via
      `webview2_vendor` / `webview2_product` on `WebEditorConfig`)
      still resolves to `%LOCALAPPDATA%\VulpusLabs\VXN1\WebView2`.

## Notes

vxn-1's `build_params_json` / `build_subdivisions_json` /
`strip_esm_exports` / `descriptor_to_json` callsite use stays in
vxn-1 vxn-ui-web's `splice_html`. `descriptor_to_json` itself is now
in vxn-core-ui-web (pub helper); the splicer calls it.
`build_subdivisions_json` indexes `vxn_app::sync::SUBDIVISIONS` — that
table can also re-export from `vxn_core_utils::sync`, but the
serialisation shape stays per-synth.
