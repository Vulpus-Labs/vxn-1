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

## Close-out (2026-08-27)

**Scope taken: full adoption**, not the vxn-1b-only fallback. The Notes ask
which was chosen — all three products are on the shared crate, and the ticket's
"changes vxn-1 and vxn-2" now reads as vxn-2 and vxn-3 (vxn-1 having gone to
`archive/`).

Re-measured, as the amendment asks. Before: 2,278 lines of xtask (vxn-1b 1105,
vxn-2 969, vxn-3 204) and 355 of `gui.rs` (131/119/105). After: 1,136 of xtask
(562/431/143) behind an 892-line shared crate, and 192 of `gui.rs`
(74/64/54).

### A prerequisite that was not there

`bundle.yml` still had `working-directory: vxn-1` on two jobs. vxn-1 was
archived in f90b67d, so both had been failing on every push, and **vxn-2 — which
ships — had never had a bundle job at all**. The one product CI covered was
vxn-1b. Fixed first and in its own commit (4337456), because "0321 lands first
so CI is watching before the bundler moves" is the whole reason this ticket
depends on it, and it was not true.

### The regression the extraction caused, and how it was found

The first pass silently dropped vxn-2's `Contents/Resources/` staging — the
`VXN2_DEV_ASSETS=1` hot-reload path. It was found by reading the
pre-extraction `bundle()`, **not** by any check: `--format clap,vst3
--universal` succeeded, produced a fat binary, and its VST3 passed the
non-hollow grep. It is now `Product::resources_dir`, and a
declared-but-missing directory is an error rather than an empty `Resources/`
that only bites whoever sets the env var. This is the ticket's "naive
extraction flattens and breaks" warning, landing exactly where it said.

### Deliberately not flattened

Each is a `Product` field or an enum, and each is pinned by a test:

- **The Info.plist version.** Reading `env!("CARGO_PKG_VERSION")` inside the
  shared crate would stamp *its* version into every bundle — vxn-1b ships 0.0.x
  and the other two ride the workspace 0.1.x, so all three plists would have
  silently become wrong.
- **`LSMinimumSystemVersion`.** vxn-1b declares 11.0.0; vxn-2 and vxn-3 declare
  10.13.0. Unifying them would change what hosts believe about two products.
- **The profile.** vxn-2 has a real debug path; 0311 removed vxn-1b's no-op
  `--release`.
- **VST3 itself.** `Option<Vst3>`, so vxn-3's `--format vst3` gets *"VXN3 has no
  VST3 build — it is CLAP-only (no wrapper project)"* rather than CMake failing
  to find a project.
- **`build_wasm` / `serve_dist` / `_headers`.** Listed among the near-verbatim
  fifteen and genuinely are not: vxn-1b serves through `node serve-coep.mjs`,
  vxn-2 through a python script, their headers differ by a
  `Cross-Origin-Resource-Policy` line, their RUSTFLAGS handling differs, and
  vxn-3 has no web build. Unioning two different tools that share a verb buys a
  parameter per difference and a home for neither.

### `gui.rs`

Not the generic `WebviewGui<E: EditorFactory>` the Design sketches. The three
diverge in ways a trait needs an associated item for each of: vxn-2 and vxn-3
hold `self.host` as an `Option` and vxn-1b does not, the `open_editor` calls
take different arguments, and only vxn-1b tears down scope capture with the
window. What moved is the ceremony —
[`parent_pointer`](../../crates/vxn-core-clap/src/gui.rs) (the per-OS branch
whose absence is the Windows "no UI" bug),
`impl_fixed_size_gui_boilerplate!` (the nine no-decision `PluginGuiImpl`
methods), and `WEBVIEW_TIMER_PERIOD_MS` (declared three times with three
comments saying the same thing). Dimensions and names stay per-product.

### Fold-ins

`clap_install_dir` / `vst3_install_dir` are one `user_plugin_dir` with three
suffixes. `bundle()` no longer returns a `PathBuf` two call sites discard.
`run_formats(&formats, |fmt| uninstall(fmt))` is `run_formats(&formats, |fmt|
PRODUCT.uninstall(fmt))` — still a closure, because it now borrows a const, but
no longer one wrapping a bare function pointer for nothing.

### Behaviour changes, flagged

- **vxn-2's `install` / `uninstall` take `--format`** and work wherever the
  format does. They were macOS-only and CLAP-only: a VST3 install had to go
  through `bundle --install`, and `uninstall` could not remove a VST3 at all.
  This is vxn-1b's dispatch, which is why the ticket says to take its variant
  as the base. Own commit (b6907b5).
- **vxn-3's CLAP stages to `target/bundled/`** rather than `target/release/`,
  matching the other two. Its only references were its own help and the CI job
  added an hour earlier; both updated. Own commit (2537c49).
- **vxn-3's `install` no longer gates on mtime.** It never usefully did.
- **`create` is stricter everywhere**, by accident of sharing: all three refuse
  a floating window through the same predicate `is_api_supported` uses, where
  each previously spelled the condition out again and could drift from its own
  gate.

### Verification

- **`workspace_root` per product**, as the acceptance asks: each xtask has its
  own test asserting the result holds the workspace `Cargo.toml` *and* that
  product's crates. Two `.parent()` calls are right for all three flat layouts
  today, and were wrong here once ([[vxn2-xtask-flat-workspace]]).
- **macOS bundling, by hand, all three.** vxn-1b and vxn-2:
  `bundle --format clap,vst3 --universal` → `lipo -info` reports `x86_64 arm64`,
  the VST3 embeds its bundle id, and the plist carries the product's own
  executable, identifier, version and macOS floor. vxn-3: `bundle` → a CLAP
  whose binary embeds `labs.vulpus.vxn3` with the 10.13.0 floor kept.
- **`install` / `uninstall`, by hand, all three**, run against a redirected
  `HOME` so the user's real `~/Library/Audio/Plug-Ins` was never touched.
  vxn-1b and vxn-2 installed and removed both formats; vxn-3 its CLAP. Nothing
  left behind, and the real directories verified unchanged afterwards.
- **`web`** for vxn-1b and vxn-2; `--help` for all three.
- `cargo test --workspace`: **1389 pass, 0 fail** (was 1374).

### Not verified here, and why

**Windows.** There is no Windows machine in this loop, so the x86_64 CLAP + VST3
path for vxn-1b and vxn-2 is covered only by `bundle.yml`'s two Windows jobs
— including the `/WHOLEARCHIVE` + `/INCLUDE:clap_entry` link and the non-hollow
check that exists because that exact configuration once shipped empty
([[vxn-windows-vst3-optref-strip]]). Those jobs run on the next push. This is
the ticket's own risk ("can break bundling for all of them at once") reduced to
one platform, and it is why the CI fix landed first and separately.

**vxn-3 has no VST3 and no Windows path**, so the acceptance line "all three
products bundle CLAP and VST3 on macOS and Windows" is unachievable as written
— it was drafted when the third seat was vxn-1. vxn-3 bundles CLAP on macOS,
CI checks it, and `Product::vst3: None` makes the absence a stated fact rather
than a gap.
