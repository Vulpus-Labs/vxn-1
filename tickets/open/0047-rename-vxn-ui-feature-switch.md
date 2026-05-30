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

- [ ] `crates/vxn-ui/` → `crates/vxn-ui-vizia/`. Cargo.toml package
      name + workspace member updated; `workspace.dependencies`
      renamed.
- [ ] `vxn-clap` features:
      ```toml
      [features]
      default = ["vizia"]
      vizia   = ["dep:vxn-ui-vizia"]
      webview = ["dep:vxn-ui-web"]
      ```
- [ ] cfg-gated `use vxn_ui_vizia as vxn_editor;` /
      `use vxn_ui_web as vxn_editor;` (same pattern the prototype
      used; this time it sticks).
- [ ] xtask + deploy.sh accept `--webview`, pass
      `--no-default-features --features webview`.
- [ ] Both `./deploy.sh` and `./deploy.sh --webview` produce
      working bundles.
- [ ] `cargo test --workspace` passes with default features and
      with `--no-default-features --features webview`.
- [ ] Default stays Vizia until E011 closes the seam (preset
      browser + keys panel + text input). After that ticket, flip
      default to webview.

## Notes

The prototype already did the feature-switch wiring once; the
mechanical bits are known to work. The new piece is the rename, which
touches every `vxn-ui::` qualifier in the codebase. Use `cargo check`
to find them all; the compiler is the source of truth.

Don't touch memory files (`vxn1-vizia-no-click-slop` etc.) — they're
still accurate about the Vizia editor's bugs; they're just under a
new crate name. Memory references are conceptual, not paths.
