---
id: "0116"
title: CI — run cargo test --workspace + vitest on every push
priority: high
created: 2026-06-10
epic: E021
---

## Summary

No CI workflow runs any tests. The three existing workflows
(`build-windows.yml`, `docs.yml`, `release.yml`) build
bundles and docs only, so 380+ Rust tests and the 143-case
vitest suite never run automatically — regressions in dsp,
engine, or the shared core crates can land on main unseen.
The JS suite is additionally gated behind the opt-in
`VXN_JS_TESTS=1` env var, one forgotten variable away from
invisible.

Add a test workflow covering the whole workspace. vxn-2
ticket 0070 asks for the same thing scoped to vxn2-*; land
this as one workflow satisfying both tickets rather than two
overlapping jobs.

## Acceptance criteria

- [ ] New `.github/workflows/test.yml` triggering on push
      and pull_request to main.
- [ ] Runs `cargo test --workspace` on a macOS runner
      (primary dev/runtime target; NEON paths compile and
      run). A Linux runner may be added for speed but does
      not replace the macOS job.
- [ ] Runs the vxn-1 JS suite: `npm ci` + vitest in
      `vxn-1/crates/vxn-ui-web/assets/` (directly or via
      `VXN_JS_TESTS=1 cargo test -p vxn-ui-web`).
- [ ] Any test failure fails the workflow; workflow status
      visible on PRs.
- [ ] Reasonable caching (`Swatinem/rust-cache` or
      equivalent) so the job is not prohibitively slow.
- [ ] vxn-2 ticket 0070 is closed by this work or explicitly
      re-scoped, not left duplicating it.

## Notes

`cargo test --workspace` covers vxn-core-*, vxn-1 and vxn-2
crates in one invocation — the flat workspace (E001) makes
this cheap. Keep bundle workflows unchanged; this ticket is
tests only.

Land early in the epic: 0117's smoothing tests, 0118's JS
tests and 0121's new unit tests all gain value from running
in CI from the moment they merge.

## Closure (2026-06-12)

Added `.github/workflows/test.yml`: macOS runner,
`cargo test --workspace` (VXN_JS_TESTS=1 un-gates the vxn-ui-web
Vitest suite via `npm ci` in the assets dir), plus
`cargo bench --no-run --workspace`. Triggers on push + PR to main,
`Swatinem/rust-cache` for caching. One workflow satisfies both this
ticket and vxn-2 0070 — `--workspace` runs vxn-core-*, vxn-1 and
vxn-2 crates together, so shared-core edits exercise vxn-2 tests
without a path filter. Closes 0070.
