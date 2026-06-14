---
id: "0027"
product: vxn-2
title: "Vendor clap-wrapper + minimal CMake scaffold"
priority: medium
created: 2026-06-13
epic: E014
depends: []
---

## Summary

First ticket of [E014](../../epics/open/E014-standalone-builds.md).
Bring in [`free-audio/clap-wrapper`](https://github.com/free-audio/clap-wrapper)
and stand up a minimal CMake project that can invoke
`target_add_standalone_wrapper` against a CLAP **static archive**
(bundled mode).

**Shares scaffold with vxn-1 E010 (VST3).** E010 0009/0010 vendor the
same `vendor/clap-wrapper` submodule + author `wrapper/CMakeLists.txt`
for VST3. If E010 lands first, this ticket shrinks to "extend the shared
scaffold to the standalone target + add `vxn2-clap` staticlib." Do not
vendor a second copy.

## Design

- Vendor clap-wrapper as a git submodule (pinned tag), shared with
  E010. It transitively fetches RtAudio / RtMidi.
- Add `crate-type = ["cdylib", "rlib", "staticlib"]` to `vxn2-clap` and
  smoke-link that clack's entry macro exports `clap_entry` from the
  archive (mirrors E010 0008 for `vxn-clap`).
- Author / extend the shared `wrapper/CMakeLists.txt` so it can invoke
  `target_add_standalone_wrapper(...)` against the static archive in
  **bundled / single-binary mode** (no runtime `.clap` to locate).
- Confirm the CMake configures and the standalone target type resolves
  on macOS first (the proving ground is 0028).
- Document the toolchain prereqs (CMake ≥ 3.21, a C++ compiler).

## Acceptance

- clap-wrapper vendored at a pinned rev (shared with E010).
- `vxn2-clap` exposes a `staticlib` whose `clap_entry` symbol links.
- `cmake` configures the standalone scaffold without error on macOS.
- README/notes capture the toolchain prereqs and the static-link
  contract.
