---
id: "0078"
title: Wire JS test suite into cargo test or CI
priority: high
created: 2026-06-01
epic: E015
---

## Summary

Decide and implement how the Vitest suite from 0077 gates merges.
Two viable paths; pick one in this ticket.

**Option A — `cargo test` drives it.** A `#[test]` in
[crates/vxn-ui-web/src/lib.rs](../../crates/vxn-ui-web/src/lib.rs)
shells `npm test --silent` under `cfg(not(miri))`. `#[ignore]`
by default when the `VXN_JS_TESTS=1` env var is unset (so a
Rust-only developer who hasn't installed Node still sees a green
`cargo test`); CI sets the var so the gate is real.

**Option B — Independent CI job.** GitHub Actions adds a `js-tests`
job that runs `npm ci && npm test` in
`crates/vxn-ui-web/assets/`. PR merge gate includes it alongside
the existing `cargo test`. No coupling at the cargo layer.

Recommendation: **Option A**. Keeps `cargo test -p vxn-ui-web`
honest as the single command a contributor runs locally, and the
ignore-by-default behaviour means it's still ergonomic for
Rust-only work.

## Acceptance criteria

- [ ] One option (A or B) chosen and implemented. Locked in the
      Notes section of this ticket at close-out.
- [ ] **If A:**
      - `crates/vxn-ui-web/src/lib.rs` adds:
        ```rust
        #[test]
        #[cfg_attr(not(env = "VXN_JS_TESTS"), ignore)]
        fn js_suite_passes() {
            let status = std::process::Command::new("npm")
                .args(["test", "--silent"])
                .current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/assets"))
                .status()
                .expect("npm not found — install Node 20+ or unset VXN_JS_TESTS");
            assert!(status.success(), "JS suite failed");
        }
        ```
        (`cfg_attr` syntax may need adjusting — `env = "..."` isn't
        a `cfg` predicate; use a build-script-emitted `cfg(vxn_js_tests)`
        flag or a runtime `std::env::var("VXN_JS_TESTS").is_ok()`
        gate inside the test body. Pick whichever is cleanest.)
      - `crates/vxn-ui-web/README.md` (or `assets/README.md`)
        documents: `VXN_JS_TESTS=1 cargo test -p vxn-ui-web` runs
        the JS suite; `cargo test -p vxn-ui-web` alone runs only
        the Rust substring suite + parser tests.
      - CI workflow sets `VXN_JS_TESTS=1` for the relevant job.
- [ ] **If B:**
      - A `.github/workflows/js-tests.yml` (or addition to the
        existing workflow) runs `npm ci && npm test` under
        `crates/vxn-ui-web/assets/` on `pull_request` and `push
        to main`.
      - PR settings (or branch protection) include the new job in
        the required-checks list.
      - `cargo test -p vxn-ui-web` is unchanged.
- [ ] In either case: `crates/vxn-ui-web/assets/README.md` updated
      with the command a contributor runs to reproduce CI locally
      (`npm test` for both options).

## Notes

Option A's tradeoff: `cargo test -p vxn-ui-web` now has an
optional external dep on `npm`. Mitigated by the `#[ignore]`
default. Local feedback loop is one command (`VXN_JS_TESTS=1 cargo
test -p vxn-ui-web`), which matches how the substring suite is
already invoked.

Option B's tradeoff: two commands to reproduce CI locally;
contributors who touch JS without touching Rust still need to
remember `npm test`. Lighter weight overall.

If CI doesn't exist yet (the repo doesn't have `.github/workflows/`),
default to Option A — the substring suite is the existing pin
and the JS suite slots beside it the same way.

## Decision

**Option A** chosen. `.github/workflows/` currently holds only a
release-on-publish workflow (no PR or push-to-main gate), so the
recommendation's "no CI" fallback applies. `cargo test -p vxn-ui-web`
stays the single local command; `VXN_JS_TESTS=1` opts the JS suite in.
A future PR-gating workflow sets the env var in the relevant job.
