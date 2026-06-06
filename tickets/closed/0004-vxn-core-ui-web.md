---
id: "0004"
title: vxn-core-ui-web — wry shell, IPC bridge, text-input popup
priority: high
created: 2026-06-06
epic: E001
---

## Summary

Extract vxn-1's `vxn-ui-web` crate (~1200 LOC) into shared
`vxn-core-ui-web`. Provides the WebView lifecycle, the JS↔Rust IPC
bridge, the batched `evaluate_script` view-event sink, and the
native text-input popup. HTML / CSS / JS assets remain per-synth —
the faceplate is each synth's identity. Implements
`EditorBackend` from `vxn-core-app`.

## Acceptance criteria

- [ ] `vxn_core_ui_web::WebEditor` — implements
      `vxn_core_app::backend::EditorBackend`. Owns a `wry::WebView`
      child window, manages open/close against a parent
      `RawWindowHandle` from the CLAP shell.
- [ ] `vxn_core_ui_web::open_editor(parent, html, on_event) -> EditorHandle`
      — constructs the WebView, mounts the supplied HTML string
      (synth supplies its own bundled assets), wires the
      `window.ipc.postMessage` JS bridge to the `on_event` closure.
- [ ] IPC inbound: JS `window.ipc.postMessage(json)` →
      `serde_json::from_str::<UiEvent>` → `on_event(UiEvent)`. Errors
      logged, not panicked.
- [ ] IPC outbound: `EditorHandle::push_event(ViewEvent)` queues into
      a per-tick batcher; `flush_events()` calls `batch_chunks` to
      dedupe by `(variant, ParamId)`, splits at a configurable byte
      cap (default 64 KB), and emits one `evaluate_script` per chunk.
      `batch_chunks` lifted verbatim from vxn-1.
- [ ] `vxn_core_ui_web::text_input::open_native_popup(initial: &str,
      on_commit: Box<dyn FnOnce(String) + Send>)` — native modal text
      input. macOS impl ports vxn-1's NSAlert / NSTextField path
      verbatim. Windows and Linux paths stub with
      `unimplemented!("vxn-core-ui-web: text input on $TARGET")` plus
      a `#[cfg]`-feature so synths can opt out on those platforms
      until impls land.
- [ ] HTML/CSS/JS bundling stays per-synth. This crate accepts the
      HTML as a `&str` (or `Cow<'static, str>`) at open time —
      no `include_str!` of vxn-1 assets in the shared crate.
- [ ] `preset_corpus_to_json(corpus: &PresetCorpus) -> String`
      helper — lifted from vxn-ui-web's existing impl. Synth-agnostic
      shape (category-grouped factory + recursive user folders).
- [ ] No deps on `vxn-1` or `vxn-2` crates. Depends on
      `vxn-core-app` (for `EditorBackend`, `UiEvent`, `ViewEvent`,
      `PresetCorpus`), `wry`, `raw-window-handle`, `serde`,
      `serde_json`, macOS bindings via `objc` / `cocoa`.
- [ ] Manual smoke test: a 50-line example binary that opens a
      WebView containing a `<button onclick="window.ipc.postMessage(
      JSON.stringify({ParamSet:{id:0,value:0.5}}))">click</button>`,
      receives the `UiEvent::ParamSet`, echoes back a
      `ViewEvent::ParamChanged`, and the JS bridge logs it. Document
      in the crate README.

## Notes

vxn-1's text-input popup is macOS-only today (vxn-1 ships macOS-first
per ADR 0004). Don't try to write Linux/Windows impls in this
ticket — stub them, file follow-up tickets. The shared crate's
contract is: macOS works, other platforms compile but panic on
`open_native_popup`.

`batch_chunks` is the critical perf bit — without it, a slider drag
produces hundreds of `evaluate_script` calls per second and the
WebView stalls. Lift the dedup-by-`(variant, ParamId)` logic and the
byte-cap loop exactly. Test fixture: a `ViewEvent::ParamChanged`
sequence of 1000 events on 10 distinct ParamIds should produce ≤ 10
output chunks.

HTML asset shape: vxn-1 currently splices CSS + 4 JS modules
(`bridge`/`browser`/`panels`/`dispatch`) into a single HTML string at
build time. That splicer stays in vxn-1's UI crate (becomes the
synth-local HTML producer). Shared crate sees only the final HTML.
vxn-2's E003 will write its own splicer for its faceplate.

The wry version + raw-window-handle version must match what vxn-1
currently pins. Confirm against vxn-1's `Cargo.toml` before adding
to `workspace.dependencies`.
