---
id: "0103"
title: "Vendor clap-wrapper + minimal CMake scaffold"
priority: medium
created: 2026-06-13
epic: E010
depends: []
---

## Summary

First ticket of [E010](../../epics/open/E010-standalone-builds.md).
Bring in [`free-audio/clap-wrapper`](https://github.com/free-audio/clap-wrapper)
and stand up a minimal CMake project that can invoke
`target_add_standalone_wrapper` against a built `.clap`.

## Design

- Vendor clap-wrapper as a git submodule or via CMake `FetchContent`
  (pin a commit). It transitively fetches RtAudio / RtMidi.
- Author a small `standalone/CMakeLists.txt` that:
  - references clap-wrapper,
  - calls `target_add_standalone_wrapper(...)` with `HOSTED_CLAP_NAME` /
    `MACOS_EMBEDDED_CLAP_LOCATION` (and the Windows equivalent) pointing
    at a built `.clap` — **embedded/hosted mode**, so no Rust changes
    and no `staticlib` crate-type.
- Confirm the CMake configures and the standalone target type resolves
  on macOS first (the proving ground is 0104).
- Document the toolchain prereqs (CMake ≥ 3.21, a C++ compiler).

## Acceptance

- clap-wrapper vendored at a pinned rev.
- `cmake` configures the standalone scaffold without error on macOS.
- README/notes capture the toolchain prereqs and the
  embedded-clap-location contract.
