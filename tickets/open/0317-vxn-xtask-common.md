---
id: "0317"
product: monorepo
title: "vxn-xtask-common: 3,015 lines of triplicated bundler across three products"
priority: medium
created: 2026-08-26
epic: E047
depends: ["0321"]
---

## Summary

`vxn-1b/xtask`, `vxn-2/xtask` and `vxn-3/xtask` are the same tool three times.
About **fifteen functions are near-verbatim** across them:

> **Amended 2026-08-27 (vxn-1 retirement).** As filed this named
> `vxn-1/xtask`, `vxn-2/xtask` and `vxn-1b/xtask`. vxn-1's went to the archive,
> so the third seat passes to `vxn-3/xtask` and the line count below is stale —
> re-measure before quoting it. The argument is unchanged and if anything
> stronger: the fork that got the best version (VXN1b's `Format`-indexed
> `install_path` / `run_formats`) is now also the canonical synth's, and the
> other two still will not receive it.

`ensure_cmake`, `ensure_msvc`, `ensure_submodules`, `ninja_available`,
`find_vst3`, `find_named_dirs`, `copy_dir_recursive`, `copy_artifact`, `io`,
`arg_value`, `parse_formats`, `lib_path`, `static_lib_path`, `build_universal`,
`serve_dist`, `run_capture`, `info_plist`, `build_wasm`.

3,015 lines total for one bundler with three name sets. VXN1b's copy is the
cleanest variant — it alone has `Format`-indexed `install_path` and
`run_formats` — which is the usual sign that the newest fork got the best
version and the older two will never receive it.

The same story, smaller, in the GUI shells:
[vxn1b-clap/src/gui.rs:6-8](../../vxn-1b/crates/vxn1b-clap/src/gui.rs#L6-L8)
admits it outright — *"Mirrors VXN1's `vxn-clap/src/gui.rs` … only the crate
names and dimensions differ"* — and a normalised diff shows ~60 of 131 lines
identical across the three copies (vxn-1 123, vxn-2 119, vxn-1b 131). The nine
trivial `PluginGuiImpl` methods and the platform parent-handle branch are pure
boilerplate.

### Why it is worth doing now rather than later

Because the bundler is where [[vxn-windows-vst3-optref-strip]] happened, and a
fix applied to one copy is a fix absent from two. The wrapper/CMake scaffold is
already shared between products ([[vxn-clap-wrapper-integration]]); the driver
that invokes it is not.

## Design

Extract `vxn-xtask-common` parameterised by a product descriptor:

```rust
pub struct Product {
    pub plugin_name:  &'static str,
    pub bundle_name:  &'static str,
    pub vst3_name:    &'static str,
    pub bundle_id:    &'static str,
    pub display_name: &'static str,
    pub lib_name:     &'static str,
    pub clap_package: &'static str,
    pub wrapper_dir:  &'static str,
    pub web_modules:  &'static [&'static str],
}
```

Each product's `main.rs` becomes its const block plus its own subcommands.
**Take VXN1b's variant as the base** — it has the `Format`-indexed dispatch the
other two lack — which means vxn-1 and vxn-2 gain behaviour in this ticket.
That is a REBASELINE-shaped change for them, and should land as its own commit
per product so a bisect can attribute a bundling regression to the right one.

For `gui.rs`: a `vxn-core-clap` generic `WebviewGui<E: EditorFactory>` or a
macro covering the parent-handle branch, timer registration and the
constant-returning methods. Dimensions and crate names stay per-product.

Fold in while there: `clap_install_dir` / `vst3_install_dir`
([xtask/src/main.rs:243-271](../../vxn-1b/xtask/src/main.rs#L243-L271)) are the
same three-branch HOME/LOCALAPPDATA/HOME shape twice, differing only in the
joined suffix; `bundle()` returns a `PathBuf` both call sites discard with
`.map(|_| ())`; and `run_formats(&formats, |fmt| uninstall(fmt))` wraps a
function pointer in a closure for no reason.

### Watch out

- `workspace_root` needs **two** `.parent()` calls in the flat-workspace layouts
  and one elsewhere — [[vxn2-xtask-flat-workspace]]. This is exactly the sort of
  per-product difference that a naive extraction flattens and breaks.
- vxn-2's `install` must always re-bundle; its mtime gate never fires, same
  memory.
- Editing factory preset TOMLs does not trigger a rebuild
  ([[vxn2-include-dir-no-rerun]]) — if the shared crate touches the bundling of
  those, the workaround has to move with it.

## Acceptance criteria

- [ ] One `vxn-xtask-common` crate; three thin `main.rs` files.
- [ ] All three products bundle CLAP and VST3 on macOS and Windows, verified by
      [[0321]]'s expanded `bundle.yml` — including the non-hollow
      `strings | grep` check for each.
- [ ] vxn-1 and vxn-2 gain VXN1b's `Format`-indexed dispatch, in their own
      commits, flagged as behaviour changes rather than pure refactors.
- [ ] `gui.rs` boilerplate exists once; per-product dimensions and names stay
      per-product.
- [ ] `xtask web`, `install`, `uninstall` and `bundle` all still work for all
      three, checked by hand once each — CI covers bundle but not install.
- [ ] `workspace_root` resolves correctly for every product; a test per product
      asserting it lands on the directory containing that product's `Cargo.toml`.

## Notes

- Deliberately depends on [[0321]]. Moving the bundler before CI builds all
  three products' bundles on every push is how you find out in six weeks.
- This is the only ticket in [[E047]] that changes vxn-1 and vxn-2. If that is
  unwelcome scope, the fallback is to extract the crate and adopt it in VXN1b
  only, leaving the other two on their copies — which keeps the duplication but
  at least stops it growing. Say which was chosen in the close-out.
- Stage explicit paths — [[vxn-concurrent-vxn2-work-no-git-add-all]].
