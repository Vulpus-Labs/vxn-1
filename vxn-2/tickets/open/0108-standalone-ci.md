---
id: "0108"
title: "CI: standalone artifacts (mac + win) for both synths"
priority: low
created: 2026-06-13
epic: E010
depends: ["0105", "0107"]
---

## Summary

Sixth ticket of [E010](../../epics/open/E010-standalone-builds.md).
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
- Plugin-only workflows (E009 0100, vxn-1's build-windows) remain
  intact.
