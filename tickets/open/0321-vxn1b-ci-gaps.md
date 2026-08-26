---
id: "0321"
product: monorepo
title: "CI: VXN1b's node suites run nowhere and its bundle only builds on a release tag"
priority: high
created: 2026-08-26
epic: E047
depends: []
---

## Summary

Two gaps, both found by reading the workflows rather than the code.

**First, what is *not* broken:** VXN1b's Rust crates are all workspace members
([Cargo.toml:40-47](../../Cargo.toml#L40-L47)), so `cargo test --workspace`
already covers them. The gap is JS-only, plus the bundle.

### 1. 435 JS tests execute nowhere

Two separate suites, neither reachable from CI:

- **`node --test`, 140 tests.**
  [`vxn-1b/crates/vxn1b-wasm/web/*.test.mjs`](../../vxn-1b/crates/vxn1b-wasm/web/) —
  eleven files covering the event ring, the codec golden table, the coordinator,
  the controller, telemetry, persistence, the faceplate bridge's routing table
  and the boot queue. No workflow invokes `node --test` anywhere in the repo;
  the only record of how to run them is
  [WIRE-FORMAT.md:220](../../vxn-1b/crates/vxn1b-wasm/web/WIRE-FORMAT.md#L220).
- **Vitest, 295 tests across 38 files.**
  [`vxn1b-ui-web/assets/__tests__/`](../../vxn-1b/crates/vxn1b-ui-web/assets/__tests__/)
  has a `package.json`, a lockfile and a vitest config — but unlike
  `vxn-ui-web`, `vxn1b-ui-web` has **no `js_suite_passes` gate**, so
  `cargo test --workspace` walks straight past it. vxn-1's gate
  ([vxn-ui-web/src/lib.rs:1567](../../vxn-1/crates/vxn-ui-web/src/lib.rs#L1567))
  is the only one in the repo.

Both suites pass today (verified 2026-08-26) — this is a wiring gap, not a
backlog of failures.

The sharpest instance is `wasm-agreement.test.mjs`, which exists specifically to
catch Rust↔JS param-count drift, and whose header records that when drift last
happened *"the runtime handshake caught it immediately — nobody ran it."* It is
still nobody-run. Note it needs a real wasm artifact and **fails rather than
skips** without one ([[0295]]), so the job has to build first.

### 2. The VST3 link path is only exercised on a release tag

[bundle.yml:23-104](../../.github/workflows/bundle.yml#L23-L104) sets
`working-directory: vxn-1` in both jobs. VXN1b's wrapper build — CMake glue,
`force_load`, `/WHOLEARCHIVE`, `/INCLUDE:clap_entry` — runs only from
[release.yml:294,364](../../.github/workflows/release.yml#L294) on a
`vxn-1b-*` tag ([[vxn-release-process]]).

That is exactly the configuration that already shipped broken:
[[vxn-windows-vst3-optref-strip]] — `/OPT:REF` stripped the whole-archived
staticlib out of every Windows `.vst3` before 2026-08-24, **the link succeeded
silently**, and it was caught by inspecting a shipped artifact, not by a build
failure. `release.yml` now has a `strings | grep labs.vulpus.vxn1b` non-hollow
check; `bundle.yml` has neither the check nor the build.

## Design

Mirror what vxn-1 already has, rather than inventing anything.

**test.yml** — two different mechanisms, because the two suites have different
prerequisites:

- *Vitest* has none, so it follows vxn-1's established idiom: add a
  `js_suite_passes` gate to `vxn1b-ui-web` keyed on the **same** `VXN_JS_TESTS`
  var, and it rides the existing `cargo test --workspace` step for free. One env
  setting un-gates both products.
- *`node --test`* needs a wasm artifact, which cargo cannot express as a test
  dependency, so it stays an explicit step — `cargo build -p vxn1b-wasm
  --target wasm32-unknown-unknown --release` (the exact command
  `wasm-agreement.test.mjs` prints in its own failure message) followed by
  `node --test`. The agreement suite **fails rather than skips** without the
  artifact ([[0295]]), which is what makes the ordering safe to rely on.

**bundle.yml** — add macOS and Windows jobs running
`cargo run -p vxn1b-xtask -- bundle --format clap,vst3`, each followed by the
same `strings | grep labs.vulpus.vxn1b` assertion `release.yml` performs. The
point is not the artifact; it is that a hollow module fails the build on the
commit that hollowed it.

## Acceptance criteria

- [ ] `node --test` over `vxn1b-wasm/web/` runs on every push, with the wasm
      built first, and reports 0 skipped.
- [ ] The `vxn1b-ui-web/assets` Vitest suite runs on every push, via a
      `js_suite_passes` gate matching vxn-1's.
- [ ] `bundle.yml` builds VXN1b CLAP + VST3 on macOS and Windows.
- [ ] A deliberately hollowed VST3 fails that job — verify once by temporarily
      dropping `/INCLUDE:clap_entry` locally, or by asserting the check catches
      an empty archive, then revert.
- [ ] No change to the vxn-1 or vxn-2 jobs' behaviour or runtime beyond the
      added VXN1b work.

## Notes

- **Renumbered 0309 → 0321 on 2026-08-26.** A concurrent session took the same
  next-id for [[0309]] (shared CPU meter) minutes apart; that one won the number
  because its id is baked into source comments across two products, while this
  one was referenced only in worklist docs. Two commits pushed before the
  renumber still say `(0309)` in their subject —
  `ci(vxn-1b): run the JS suites and bundle VXN1b on every push (0309)` and the
  bundle-workflow header — and mean *this* ticket. Nothing else does.

- Do this **before** the rest of [[E047]]. Every other ticket in the epic
  deletes something, and deleting without CI watching is how a shipped bundle
  loses a file nobody notices for six weeks.
- Both suites were run locally before wiring and both are green (140 + 295), so
  the first CI run should not be a surprise. If it is, the difference is
  environmental — a missing wasm artifact or a Node version gap — not a real
  regression.
- `bundle.yml` had **no** non-hollow check for vxn-1 either, only `release.yml`
  did. Added for both products here rather than leaving the new VXN1b jobs
  better-guarded than the existing vxn-1 ones; vxn-2 has no `bundle.yml` job at
  all, which is a separate gap and out of scope.
- One `cargo test` at a time — [[vxn-no-parallel-cargo-test]]. Stage explicit
  paths — [[vxn-concurrent-vxn2-work-no-git-add-all]].
