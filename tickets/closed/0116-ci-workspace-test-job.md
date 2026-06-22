---
id: "0116"
product: vxn-1
title: CI — workspace cargo test + gated vitest suite on every push
priority: high
created: 2026-06-10
epic: E011
---

## Summary

The review noted the 380 Rust + 143 JS tests had no CI gate — nothing
ran them on push. Add a CI job running `cargo test --workspace` plus the
gated vitest suite (`VXN_JS_TESTS=1`), coordinated with vxn-2 ticket 0070
so one workflow covers the whole flat workspace.

## Acceptance criteria

- [ ] CI runs `cargo test --workspace` on every push and PR to `main`,
      failing the build on any test failure.
- [ ] The vitest faceplate suite runs under `VXN_JS_TESTS=1` (un-gating
      the `js_suite_passes` shell-out), with Node + `npm ci` set up.
- [ ] Bench code is compile-checked (`cargo bench --no-run`) so bit-rot
      fails the build without running criterion in CI.

## Notes

Scaffolded retroactively (2026-06-22): the workflow landed alongside
vxn-2's 0070 and was never given a vxn-1 ticket. This file records and
verifies the state for the E011 trail.

## Close-out (2026-06-22)

Verified done — [.github/workflows/test.yml](../../.github/workflows/test.yml)
covers it:

- `cargo test --workspace` with `VXN_JS_TESTS: "1"` on `push` and
  `pull_request` to `main` (plus `workflow_dispatch`); `--workspace`
  covers vxn-core-*, all vxn-1 and all vxn-2 crates in one invocation
  (flat workspace).
- Node 20 + `npm ci` against
  `vxn-1/crates/vxn-ui-web/assets/package-lock.json`; the `VXN_JS_TESTS`
  env un-gates `js_suite_passes`, which shells `npm test` (Vitest/jsdom).
- `cargo bench --no-run --workspace` compile-checks the benches.
- Concurrency group cancels superseded runs on the same ref. Runs on
  `macos-latest` (primary target: NEON compiles/runs, macOS-only xtask
  bundler, macOS-gated `editor_smoke.rs`).
