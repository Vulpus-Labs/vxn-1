---
id: "0100"
title: "build-windows.yml CI workflow for vxn-2"
priority: high
created: 2026-06-13
epic: E009
depends: ["0098", "0099"]
---

## Summary

Third ticket of [E009](../../epics/open/E009-windows-parity.md). Add a
`build-windows.yml` GitHub Actions workflow for vxn-2, mirroring
vxn-1's (`.github/workflows/build-windows.yml`).

## Design

- Mirror vxn-1's workflow: `workflow_dispatch` + push to `main`,
  `windows-latest`, `dtolnay/rust-toolchain@stable`,
  `Swatinem/rust-cache@v2`, then `cargo xtask bundle --release` in the
  `vxn-2` working dir.
- Upload artifact `VXN2-windows-x64` pointing at
  `target/bundled/VXN2.clap` (the path 0098 aligns to) with
  `if-no-files-found: error`.
- Decide whether this is a separate `vxn-2`-named workflow file or a
  matrix addition — vxn-1 and vxn-2 keep separate pipelines today, so a
  dedicated `build-windows-vxn2.yml` (or rename for clarity) is the
  low-surprise choice.

## Acceptance

- Workflow runs on push to `main`, builds `VXN2.clap` on
  `windows-latest`, and uploads `VXN2-windows-x64`.
- Green run on the branch before merge.
- vxn-1's existing Windows workflow is untouched.
