---
id: "0115"
title: Host-boundary panic safety — wry build expect → Result
priority: high
created: 2026-06-10
epic: E021
---

## Summary

`vxn-ui-web::open_editor` panics on WebView construction
failure: `.build().expect("wry WebView build failed")`
(`vxn-ui-web/src/lib.rs:241`). The call site is
`vxn-clap/src/gui.rs:92` inside `PluginGuiImpl::set_parent`,
which is invoked by the host through the CLAP `gui`
extension. With `panic = "unwind"` the panic unwinds through
the `extern "C"` frame into the host — undefined behaviour
per Rust's FFI rules, regardless of whether common hosts
happen to survive it. `set_parent` already returns
`Result<(), PluginError>`; the panic just needs to become an
error on that path.

While in there, audit the remaining panic paths reachable
from CLAP entry points and decide each one explicitly.

## Acceptance criteria

- [ ] `open_editor` (and the shared
      `vxn_core_ui_web::open_editor` it delegates to) returns
      `Result` instead of panicking on `WebViewBuilder::build`
      failure; `set_parent` maps it to `PluginError`.
- [ ] A forced build failure (e.g. invalid parent handle in a
      test harness, or temporarily stubbing the builder)
      leaves the plugin alive: audio continues, a later
      `set_parent` retry is possible, no unwind crosses the
      C ABI.
- [ ] Audit pass over `vxn-clap` + `vxn-ui-web` for
      `unwrap`/`expect`/`panic!` reachable from host-facing
      entry points (`process`, `flush`, `save`/`load`, gui
      extension, timers). Each remaining one is either
      converted or carries a comment stating why it is
      statically unreachable (the
      `from_index(...).expect("bound by enum")` class is
      acceptable with the comment).
- [ ] The Windows `ensure_webview2_data_dir` `unsafe
      set_var` (`vxn-ui-web/src/lib.rs:46-57`) gets a comment
      documenting the single-threaded-at-init assumption, or
      is replaced with a thread-safe mechanism.
- [ ] `cargo test --workspace` green; editor smoke tests
      still pass on macOS.

## Notes

vxn-2's `vxn2-ui-web` delegates to the same shared
`open_editor` — fixing the shared function fixes both
synths; the vxn-2 side needs no separate ticket, just a
compile check.

This is the only UB-class finding of the review. Everything
else in E021 is quality or hygiene.
