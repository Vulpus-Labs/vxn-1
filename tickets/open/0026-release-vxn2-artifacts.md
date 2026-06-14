---
id: "0026"
product: vxn-2
title: "release.yml vxn-2 artifacts (mac-universal + win-x64)"
priority: low
created: 2026-06-13
epic: E013
depends: ["0024"]
---

## Summary

Stretch ticket of [E013](../../epics/open/E013-windows-parity.md).
Extend release packaging to produce vxn-2 artifacts on tagged releases,
mirroring vxn-1's `release.yml` (macOS-universal `.clap.zip` +
Windows-x64 `.clap`).

## Design

- Mirror vxn-1's `release.yml` jobs for vxn-2:
  - macOS: `cargo xtask bundle --release --universal` →
    `ditto`-zipped `VXN2-macOS-universal.clap.zip`. (Requires the
    universal/`lipo` path to exist in vxn-2's xtask — vxn-1 has
    `build_universal`; vxn-2 may need it ported. Scope-check before
    committing — may split into its own ticket.)
  - Windows: `cargo xtask bundle --release` → `VXN2-windows-x64.clap`.
- Gate on tag push, attach to the GitHub release.

## Acceptance

- Tagged release produces `VXN2-macOS-universal.clap.zip` and
  `VXN2-windows-x64.clap`.
- vxn-1's release flow is untouched.

## Notes

Stretch / deferrable — the core E013 deliverable is the
`build-windows.yml` CI artifact (0024), not release packaging. Port of
the macOS universal/`lipo` path may warrant its own ticket if vxn-2's
xtask lacks it.
