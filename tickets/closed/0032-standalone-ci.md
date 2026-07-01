---
id: "0032"
product: vxn-2
title: "CI: standalone artifacts (mac + win) for both synths"
priority: low
created: 2026-06-13
epic: E014
depends: ["0029", "0031"]
---

## Summary

Sixth ticket of [E014](../../epics/open/E014-standalone-builds.md).
Generalise the standalone build into CI so mac + win standalone
artifacts are produced for vxn-1 and vxn-2.

## Design

- Extend the build workflows (or add a `build-standalone.yml`) to:
  - install CMake + the C++ toolchain (mostly present on the runner
    images),
  - run the `xtask standalone` / CMake target on `macos-14` and
    `windows-latest` for each synth,
  - upload `vxn{1,2}-standalone-macOS` and `VXN{1,2}-windows-x64`
    standalone artifacts.
- Decide matrix vs per-synth-file consistent with the existing split.

## Acceptance

- CI produces standalone artifacts for both synths on mac and win.
- Green runs before merge.
- Plugin-only workflows (E013 0024, vxn-1's build-windows) remain
  intact.

## Close-out (2026-07-01)

- [.github/workflows/build-standalone.yml](../../.github/workflows/build-standalone.yml): new workflow; triggers on push/PR to `main` + `workflow_dispatch`.
  - `macos` job (`macos-14`): VXN1 `cargo xtask standalone --release --universal` + VXN2 `cargo xtask standalone --release`; `brew install ninja`; uploads `vxn1-standalone-macOS` (`target/bundled/VXN1.app`) and `vxn2-standalone-macOS` (`target/bundled/VXN2.app`) with `if-no-files-found: error`.
  - `windows` job (`windows-latest`): `ilammy/msvc-dev-cmd@v1`, `choco install ninja`, VXN1 + VXN2 standalone; uploads `VXN1-windows-x64-standalone` and `VXN2-windows-x64-standalone`.
  - Both jobs: `submodules: recursive`, `Swatinem/rust-cache@v2`.
- Plugin-only workflows (`build-windows-vxn2.yml`, `build-windows.yml`, `build.yml`) unchanged.
