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
- **The first real run was informative, as predicted — it caught an incomplete
  step of mine.** `node --test` needs TWO wasm artifacts; the first cut of the
  build step made only `vxn1b-wasm`, so the three suites that drive
  `vxn1b_web_controller.wasm` (controller, faceplate-bridge, persistence)
  aborted. CI reported **91 tests where a complete run has 151** — and the step
  did fail, because those suites fail rather than skip on a missing artifact
  (0295). Without that rule it would have gone green at 60% coverage. Fixed by
  building via `cargo run -p vxn1b-xtask -- web`, which owns the crate list, so
  a third wasm crate cannot silently fall out of it.

  Residual risk worth knowing: the step's health is judged by exit code, not by
  test count. A future change that stops a suite from running *without* tripping
  the fail-don't-skip rule would show as green with fewer tests. A count floor
  would catch it, at the cost of churn every time tests are added.

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

## Close-out (2026-08-27)

Both gaps closed and **proven on the remote**, not just locally. Green run:
`d992dc6`.

### JS suites now run on every push

`Test` workflow, all 11 steps green:

- **`node --test`: `# tests 151 · pass 151 · fail 0 · skipped 0`** — byte-for-byte
  the local result, so the whole suite ran rather than a subset.
- **Vitest** rides the existing `cargo test --workspace` step via a
  `js_suite_passes` gate in `vxn1b-ui-web`
  ([lib.rs](../../vxn-1b/crates/vxn1b-ui-web/src/lib.rs)) keyed on the **same**
  `VXN_JS_TESTS` var vxn-1 uses, so one env setting un-gates both products. Both
  suites' Vitest runs are visible in the log.

### Bundle now builds VXN1b on every push

All four jobs green — `macOS vxn-1b (universal)`, `Windows vxn-1b (x86_64)`, and
the two pre-existing vxn-1 jobs. Each VST3 is checked for its bundle id after
linking.

### What the first real run caught

The step failed on its first genuine execution, which was the point of opening
this. `node --test` needs **two** wasm artifacts; the first cut of the build
step made only `vxn1b-wasm`, so the three suites driving
`vxn1b_web_controller.wasm` aborted and CI reported **91 tests where a complete
run has 151**. It failed rather than passing at 60% coverage only because those
suites fail rather than skip on a missing artifact ([[0295]]) — that rule
earning its keep on its first outing.

Not visible locally: the controller wasm was sitting in `target/` from the 0307
work. Reproduced by deleting it — 91 tests, 3 fail, matching CI exactly. Now
built with `cargo run -p vxn1b-xtask -- web`, the product's own definition of
the web artifacts, so the crate list lives in one place and a third wasm crate
cannot silently fall out of it.

Two other self-inflicted failures on the way, both worth recording:

- **wasm32 was installed for the wrong toolchain.** A CI action's `targets:`
  input installs for the channel *it* resolves (stable); `rust-toolchain.toml`
  then pins 1.95.0 and cargo uses that, so the target was present on a toolchain
  nothing runs — `can't find crate for std`. The `targets` list in
  `rust-toolchain.toml` is the only place that works, and its own comment
  already said so for the macOS cross targets.
- **A `startup_failure` that was not ours.** Both `Test` and `Bundle` failed at
  startup on one commit that did not touch either file; a re-run started
  cleanly. Transient infrastructure — worth knowing before anyone debugs a
  workflow that "broke" without changing.

### Deviations from the ticket as written

- **The premise was partly wrong and was corrected before landing.** VXN1b's
  Rust crates were already workspace members, so `cargo test --workspace`
  covered them. The gap was JS-only, plus the bundle.
- **`bundle.yml` gained the non-hollow VST3 check for vxn-1 as well**, which is
  a change to a pre-existing job rather than pure addition. Leaving the new
  VXN1b jobs better-guarded than the vxn-1 ones next to them seemed the wrong
  trade. vxn-2 still has no `bundle.yml` job at all — a separate gap, untouched.
- **The hollow-VST3 check was verified by logic, not by hollowing a real
  build.** On macOS, against the actual 10.9 MB `VXN1b.vst3`, the check accepts
  the real binary and rejects a stand-in without the bundle id. The Windows form
  is `release.yml`'s verbatim, unchanged and already proven in production, but I
  have not run it against a deliberately stripped module. The acceptance
  criterion asked for that; this is the honest state of it.

### Residual risk

The step's health is judged by exit code, not test count. A change that stops a
suite running *without* tripping fail-don't-skip would go green with fewer
tests — exactly the shape of what happened here, minus the rule that caught it.
A count floor would close it, at the cost of churn whenever tests are added.
Recorded rather than decided.
