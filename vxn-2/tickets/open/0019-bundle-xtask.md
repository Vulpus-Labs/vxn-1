---
id: "0019"
title: Plugin bundling xtask
priority: medium
created: 2026-06-05
epic: E002
---

## Summary

`cargo xtask bundle` produces a host-loadable `.clap` bundle (macOS
bundle directory layout) from the `vxn2-clap` cdylib. `cargo xtask
install` copies that bundle into the user CLAP search path. Both
commands are reproducible, idempotent, and pure — no Cargo.toml
mutation, no shell scripts hiding off-tree.

Lives in a new `vxn-2/xtask` crate (binary). Mirrors VXN1's
`vxn-1/xtask` shape — copy the layout but not the install paths
verbatim (the bundle name and id differ).

## Acceptance criteria

- [ ] `vxn-2/xtask` crate added to the workspace as a `[[bin]]`
      target. Workspace `.cargo/config.toml` aliases `xtask = "run
      --package xtask --release --"` so `cargo xtask bundle` works
      from the workspace root.
- [ ] `cargo xtask bundle`:
      - Runs `cargo build --release -p vxn2-clap`.
      - Locates `target/release/libvxn2_clap.dylib`.
      - Writes a bundle directory at `target/release/vxn2.clap/` with
        layout:
        ```
        vxn2.clap/
        └── Contents/
            ├── Info.plist
            ├── MacOS/
            │   └── vxn2          (the dylib, renamed)
            └── PkgInfo           (`BNDL????`)
        ```
      - `Info.plist` carries: `CFBundleIdentifier = labs.vulpus.vxn2`,
        `CFBundleName = VXN2`, `CFBundlePackageType = BNDL`,
        `CFBundleExecutable = vxn2`, `CFBundleVersion` from the
        workspace `version`, `CFBundleSupportedPlatforms = MacOSX`.
      - Idempotent: a second run overwrites without duplicating.
- [ ] `cargo xtask install`:
      - Implies `bundle` (run first if the bundle directory is older
        than the cdylib or missing).
      - Copies `target/release/vxn2.clap/` to
        `~/Library/Audio/Plug-Ins/CLAP/vxn2.clap/`, replacing any
        previous bundle.
      - On replacement: `rm -rf` the destination then copy fresh
        (safer than per-file overwrite — drops stale files from
        prior layouts). User confirmation NOT required (xtask is a
        dev-only command; the destination namespace is ours).
- [ ] `cargo xtask uninstall`:
      - Removes `~/Library/Audio/Plug-Ins/CLAP/vxn2.clap/` if
        present; no-op if absent. Returns success either way.
- [ ] No external tooling beyond `cargo` + `std::fs`. No `plutil`,
      no `codesign` (ad-hoc unsigned is fine for local dev; signing
      is a release-engineering ticket).
- [ ] `cargo xtask --help` prints subcommands + their descriptions.
- [ ] Smoke: after `cargo xtask install`, Bitwig (or another
      installed CLAP host) discovers `VXN2` in its plugin browser
      on next rescan.

## Notes

VXN1's xtask has more surface (dev-build, dist, codesign hooks) than
this needs — keep VXN2's xtask minimal at first; add ceremony when a
specific need surfaces.

The `Info.plist` writes as an XML plist string literal in the xtask
source (no plist crate dep). That's fine — the keys are stable and
small. If the format ever grows, swap in `plist` then.

macOS only at this point. Linux/Windows bundle layouts are
mechanically different (`.so` / `.dll` next to no plist; Windows just
copies the `.dll` to `%CommonProgramFiles%\CLAP`). Adding cross-
platform targets is a follow-up — gated on someone actually running
VXN2 on Linux / Windows.

The xtask must not depend on `vxn2-clap` (only build it). Keeps the
xtask compile fast and avoids a feature-flag rebuild cycle.

If `vxn2-clap`'s `target/release/libvxn2_clap.dylib` doesn't exist
after `cargo build --release -p vxn2-clap` (e.g. cross-compile
target), the xtask should error with a clear message naming the
expected path, not panic on `unwrap`.
