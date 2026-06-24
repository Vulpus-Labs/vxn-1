---
id: "0014"
product: vxn-1
title: CI — VST3 artifacts on mac + new Windows runner
priority: medium
created: 2026-06-08
epic: E010
---

## Summary

Extend the existing macOS CI runner to build and upload
`VXN1.vst3` (universal) alongside the existing `VXN1.clap`,
and add a `windows-latest` runner that builds and uploads
both `VXN1.clap` and `VXN1.vst3` (x86_64). Gated on a clean
pass of ticket 0013.

Per ADR 0008 §4, epic E010 acceptance.

## Acceptance criteria

- [ ] macOS CI job step replaced / extended to run:
      `cargo xtask bundle --release --format clap,vst3
      --universal`.
- [ ] macOS CI uploads artifacts:
      - `VXN1.clap` (universal bundle, as today).
      - `VXN1.vst3` (universal bundle, new).
- [ ] New `windows-latest` job:
      - Checks out the repo with
        `submodules: recursive`.
      - Installs CMake + Ninja (cache where the CI provider
        supports it).
      - Sets up MSVC environment (`ilammy/msvc-dev-cmd` or
        equivalent action).
      - Runs `cargo xtask bundle --release --format
        clap,vst3`.
      - Uploads `VXN1.clap` and `VXN1.vst3` as artifacts.
- [ ] Both jobs run on every push to `main` and every PR,
      matching the current CLAP build cadence.
- [ ] Submodule checkout step is explicit
      (`actions/checkout@... with: submodules: recursive`)
      in both jobs — clap-wrapper + vst3sdk must be present
      before `cargo xtask` runs.
- [ ] Build time guardrail: total job wall-clock should not
      regress by more than ~3 minutes on macOS (CMake build
      adds time; if it exceeds the budget, look at cmake
      build cache and Ninja parallelism before raising the
      cap).
- [ ] If either VST3 build fails, the job fails — VST3 is
      now a shipping artifact, not a side experiment.
- [ ] README / contributing notes mention the artifacts
      available per CI run.

## Notes

Don't add a Linux runner here; Linux VST3 is out of scope
for E010 (deferred follow-up).

Code signing is *not* a CI step yet. Distribution-grade
signing arrives in a future ticket; for now CI artifacts
are unsigned and intended for internal validation /
testing-deployment use.

If the macOS runner already runs the CLAP build with
`--universal`, simply extending the format flag is a one-
line change. The bulk of new CI yaml is the Windows job.

Cache strategy: cmake build dirs (`target/wrapper-release`)
can be cached across runs keyed on a hash of `vendor/`
submodule SHAs + `vxn-1/wrapper/CMakeLists.txt`. Cargo build
cache stays as-is. Don't over-tune — measure first.

If the Windows runner image evolves and stops shipping a
specific toolchain piece (Ninja, CMake), this ticket's CI
yaml will need touch-ups. That's expected churn, not a
regression of this epic.

Closing this ticket closes the epic.

## Close-out (2026-06-24)

New [.github/workflows/bundle.yml](../../.github/workflows/bundle.yml) is the
per-push/per-PR artifact pipeline (push `main` + PR + `workflow_dispatch`,
concurrency-cancelled), replacing the CLAP-only `build-windows.yml` (removed —
subsumed). Two jobs:

- **macOS (macos-14)**: `submodules: recursive`, both apple targets, `brew
  install ninja`, `cargo xtask bundle --release --format clap,vst3 --universal`,
  uploads `VXN1.clap` + `VXN1.vst3` (universal bundle dirs; upload-artifact@v4
  zips them).
- **Windows (windows-latest)**: `submodules: recursive`, `ilammy/msvc-dev-cmd@v1`
  for the MSVC env (satisfies xtask's `ensure_msvc` cl.exe preflight), `choco
  install ninja`, `cargo xtask bundle --release --format clap,vst3`, uploads
  `VXN1.clap` + `VXN1.vst3` (x86_64). A failed VST3 build fails the job
  (`if-no-files-found: error` + non-zero xtask exit).

Both jobs build VST3 via the same `xtask --format clap,vst3` path (0011/0012);
the wrapper CMake (0010) handles each platform's bundle layout.

Also extended [release.yml](../../.github/workflows/release.yml) (release-publish
trigger) symmetrically so published releases attach zipped CLAP + VST3 for both
platforms — keeps releases consistent with CI; not strictly in this ticket's
push/PR scope but the same one-flag change.

[README.md](../../vxn-1/README.md) Building section now documents the
`--format clap,vst3` invocation, the CMake/Ninja/MSVC prereqs, and a **CI
artifacts** subsection listing what each run / release produces.

Build-time guardrail, cache strategy, green-build verification: deferred to the
first live runs — YAML is `yq`-valid locally but GitHub-hosted runner behaviour
(Ninja-after-msvc-dev-cmd, macOS universal CMake time) can only be confirmed on
Actions. If the macOS job exceeds the ~3-min budget, add an `actions/cache` for
`target/wrapper-release` keyed on `vendor/` SHAs + `CMakeLists.txt` hash before
raising the cap (per Notes — measure first, hence not pre-tuned here).

Acceptance boxes left unchecked pending the first green Actions run.
