---
id: E013
product: vxn-2
title: vxn-2 Windows build parity
status: open
created: 2026-06-13
---

## Goal

Bring vxn-2 to parity with vxn-1's Windows pipeline: a working
`VXN2.clap` that builds in CI on `windows-latest`, loads in a Windows
CLAP host, and opens its WebView2 HTML editor. vxn-1 already does all
of this (`.github/workflows/build-windows.yml`, cross-platform
`xtask bundle`, win32 `set_parent` branch). vxn-2 does not — its
`xtask bundle` hard-errors off macOS — so vxn-2 has never been compiled
or run on Windows.

This is a **prerequisite epic**: E014 (standalone mac + windows) needs a
Windows `.clap` to host, and other cross-platform work keys off a green
Windows build.

When this epic closes:

- `cargo xtask bundle --release` produces `VXN2.clap` on Windows
  (and Linux), not just macOS.
- A `build-windows.yml` workflow builds and uploads `VXN2-windows-x64`
  on every push to `main`, mirroring vxn-1.
- The WebView2 editor opens and renders inside a Windows CLAP host
  (no "Windows no-UI" regression — see vxn-1 `gui.rs` note), IPC works,
  and the native text-input popup accepts Enter/Esc.

## Why now

The window-handling **code already exists** and is shared, so this is
unblock-and-verify, not build-from-scratch:

- `vxn2-clap/src/gui.rs` already structurally mirrors vxn-1 and carries
  the `as_cocoa_nsview` / `as_win32_hwnd` / `as_x11_handle` branches in
  `set_parent`.
- `vxn2-ui-web` is a thin wrapper over `vxn-core-ui-web`, which already
  ships the Windows WebView2 lifecycle + the `WS_POPUP` owner-anchored
  text-input popup (`windows-sys`), and vxn-2 sets
  `webview2_vendor = "Vulpus"` / `webview2_product = "VXN2"`.

The single hard blocker is `vxn-2/xtask/src/main.rs`:

```rust
fn bundle() -> Result<PathBuf, String> {
    if !cfg!(target_os = "macos") {
        return Err("bundle currently only supports macOS".into());
    }
```

vxn-1's xtask instead branches: macOS builds a `Contents/MacOS` bundle
dir; Windows/Linux rename the shared library to `.clap` (a CLAP is just
the shared lib with a `.clap` name). vxn-2 must adopt the same shape.
Everything else is verifying code that has compiled on macOS but never
on MSVC.

## Scope

**In:**

- Cross-platform `xtask bundle`: macOS keeps the `.app`-shaped bundle
  dir; Windows/Linux produce `VXN2.clap` = the renamed `.dll` / `.so`.
  Guard the macOS-only `Contents/Resources` dev-asset staging so it is
  skipped off macOS (production reads the `include_str!` embed there).
- Align the bundle output path to `target/bundled/VXN2.clap` to match
  vxn-1's CI convention (vxn-2 currently writes `target/release/`).
- Get the full vxn-2 workspace compiling clean under MSVC on
  `windows-latest` — fix any `cfg`, path-separator, or toolchain breaks
  surfaced the first time `vxn2-*` + deps build on Windows.
- `build-windows.yml` CI workflow for vxn-2, mirroring vxn-1: build the
  bundle, upload `VXN2-windows-x64`.
- Manual verification in a Windows CLAP host that the editor opens,
  renders, IPC round-trips, and the text-input popup works. Document the
  WebView2 runtime prerequisite.

**Out:**

- The standalone executables (mac + windows) — that is E014, which
  depends on this epic.
- Windows/Linux **dev-asset hot-reload** (the `VXN2_DEV_ASSETS`
  override). vxn2-ui-web already flags this as a follow-up; production
  embeds assets, so it does not block a shippable Windows plugin.
- Linux CLAP packaging beyond what falls out of the cross-platform
  xtask branch for free (no Linux CI job in this epic).
- macOS-universal / Windows **release** artifacts in `release.yml` —
  optional stretch (ticket 0026); the core deliverable is the
  `build-windows.yml` CI artifact.

## Tickets

- [ ] [0022 — xtask: cross-platform bundle (Win/Linux renamed lib → .clap)](../../tickets/open/0022-xtask-cross-platform-bundle.md)
- [ ] [0023 — vxn-2 compiles clean under MSVC on windows-latest](../../tickets/open/0023-vxn2-windows-compile.md)
- [ ] [0024 — build-windows.yml CI workflow for vxn-2](../../tickets/open/0024-build-windows-ci.md)
- [ ] [0025 — Windows editor / window-handling verification](../../tickets/open/0025-windows-editor-verify.md)
- [ ] [0026 — release.yml vxn-2 artifacts (mac-universal + win-x64)](../../tickets/open/0026-release-vxn2-artifacts.md) *(stretch)*

## Dependency order

```text
0022 (xtask cross-platform bundle) ──┐
                                      ├─> 0024 (build-windows CI) ─> 0025 (editor verify) ─> 0026 (release)
0023 (Windows compile fixes) ────────┘
```

- 0022 and 0023 are independent and can land in parallel; 0022 is pure
  xtask logic, 0023 is whatever the first MSVC build surfaces.
- 0024 needs both: a cross-platform bundle step AND a workspace that
  compiles on Windows.
- 0025 needs a built `VXN2.clap` (from 0024's artifact or a local
  Windows build) and a Windows host.
- 0026 is a stretch packaging step that can land any time after 0024.

## Risks

- **First MSVC compile is an unknown.** vxn-2 has only ever built on
  macOS/Linux. Likely surface: `objc`-gated code in `vxn-core-ui-web`
  is `cfg(target_os = "macos")` already, but any unguarded macOS
  assumption in vxn2-* will break. Budget 0023 as discovery.
- **"Windows no-UI" bug class.** vxn-1's `gui.rs` documents that
  without the per-OS parent-handle branch the accessor returns `None`
  off-macOS and the editor silently never opens. vxn-2's `set_parent`
  has the win32 branch, but it has never executed — 0025 must confirm
  the editor actually mounts, not just that the host loads the plugin.
- **WebView2 runtime dependency.** The editor needs the Edge WebView2
  runtime. It ships by default on current Win10/11, but the verification
  must note it and confirm `ensure_webview2_data_dir` (vendor "Vulpus" /
  product "VXN2") behaves on a clean machine.
- **Text-input popup.** The `WS_POPUP` popup is owner-anchored to the
  host HWND to bypass the host's transport-key swallow. Untested on
  Windows; 0025 must confirm Enter/Esc reach the popup.
- **Bundle path divergence.** Aligning vxn-2 to `target/bundled/` may
  touch `install` / `uninstall` path helpers — keep them consistent in
  0022 so a dev's local install still works.

## Acceptance

- `cargo xtask bundle --release` produces `target/bundled/VXN2.clap` on
  macOS, Windows, and Linux. macOS output is unchanged (still the
  `Contents/MacOS` bundle dir with `Info.plist` + embedded assets).
- `cargo build -p vxn2-clap` is green on `windows-latest` with no
  macOS-only `cfg` leakage.
- `build-windows.yml` runs on push to `main`, builds `VXN2.clap`, and
  uploads `VXN2-windows-x64` with `if-no-files-found: error`.
- In a Windows CLAP host: the plugin loads, the WebView2 editor opens
  and renders the faceplate, knob/param IPC round-trips, and the
  text-input popup accepts Enter/Esc. Recorded in 0025 with the
  WebView2 runtime prereq documented.
- No regression to the macOS build, install path, or existing tests.
