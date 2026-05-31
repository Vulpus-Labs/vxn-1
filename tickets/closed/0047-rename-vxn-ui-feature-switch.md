---
id: "0047"
title: Rename vxn-ui → vxn-ui-vizia; cargo feature switch in vxn-clap
priority: medium
created: 2026-05-30
epic: E010
---

## Summary

Rename the existing `vxn-ui` crate to `vxn-ui-vizia` so the two
editor backends carry distinct names. Add a cargo feature switch at
`vxn-clap` (`vizia` / `webview`, default `vizia`) that picks the
backend at compile time. Restore deploy.sh's `--webview` flag (now as
a permanent piece of build infrastructure, not a prototype hack).

## Acceptance criteria

- [x] `crates/vxn-ui/` → `crates/vxn-ui-vizia/`. Cargo.toml package
      name + workspace member updated; `workspace.dependencies`
      renamed.
- [x] `vxn-clap` features: `default = ["vizia"]`,
      `vizia = ["dep:vxn-ui-vizia", "dep:objc"]`,
      `webview = ["dep:vxn-ui-web"]`. `objc` rides the vizia flag
      since `parent_backing_scale` is the only call site.
      `compile_error!` guards cover the "no backend" and "both
      backends" misconfigurations.
- [x] cfg-gated `use vxn_ui_vizia as vxn_editor;` /
      `use vxn_ui_web as vxn_editor;` in `crates/vxn-clap/src/lib.rs`.
      `set_parent` branches on the same cfgs since the two backends
      take different `open_editor` shapes.
- [x] xtask + deploy.sh accept `--webview`, pass
      `--no-default-features --features webview`.
- [x] Both `cargo xtask bundle` and `cargo xtask bundle --webview`
      produce bundles at `target/bundled/VXN1.clap`.
- [x] `cargo test --workspace` passes with default features;
      `cargo test -p vxn-clap --no-default-features --features
      webview` passes (the other crates have no feature-dependent
      code, so per-package suffices for the swap).
- [x] Default stays Vizia. Flip to webview happens in a later
      ticket after E011 closes the seam.

## Notes

The prototype already did the feature-switch wiring once; the
mechanical bits are known to work. The new piece is the rename, which
touches every `vxn-ui::` qualifier in the codebase. Use `cargo check`
to find them all; the compiler is the source of truth.

Don't touch memory files (`vxn1-vizia-no-click-slop` etc.) — they're
still accurate about the Vizia editor's bugs; they're just under a
new crate name. Memory references are conceptual, not paths.
