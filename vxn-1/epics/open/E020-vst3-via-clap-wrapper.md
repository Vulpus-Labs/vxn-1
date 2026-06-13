---
id: E020
title: VST3 distribution via clap-wrapper (mac + win)
status: open
created: 2026-06-08
---

## Goal

Ship VXN1 as a VST3 plugin on macOS (universal) and Windows
(x86_64), built by wrapping the existing CLAP through
free-audio/clap-wrapper in bundled single-binary mode. The
CLAP build, the engine, the parameter model, the controller
and the HTML faceplate are unchanged; VST3 is purely a
distribution artifact derived from the same source.

Per ADR 0008.

> **Coordination with vxn-2 E010 (standalone builds, 2026-06-13).**
> Both epics drive free-audio/clap-wrapper in **bundled / single-binary
> (static-link) mode** and both need the same `vendor/clap-wrapper`
> submodule + a shared `wrapper/CMakeLists.txt`. Land the vendor + CMake
> scaffold **once** (here in 0109/0110, or E010's 0103 — whichever lands
> first) and have the other reuse it. The `staticlib` crate-type change
> (0108) is the same change E010 needs for `vxn2-clap`; the link mode is
> reconciled to static across both formats and both synths, so one
> Rust-side pattern serves VST3 and standalone. The "Standalone format
> (no demand)" line under Out of scope below is **superseded** by E010.

## Background

VXN1 ships today as a CLAP only. ADR 0001 §1 pinned us to
CLAP-only because VST3 SDK was dual GPLv3/proprietary and we
wanted permissive distribution. VST 3.8 (2025-10-29) is now
MIT, dissolving that constraint.

clap-wrapper translates VST3 host calls into CLAP calls
one-for-one at runtime. Bundled mode statically links the CLAP
into the `.vst3` so the wrapper output is self-contained — no
external `.clap` dependency at install time, no install-order
coupling, friendlier to DAW validators.

The only Rust-side change is adding `staticlib` to
`vxn-clap`'s crate-types so the same source produces both the
existing CLAP cdylib and a static archive the wrapper can
link. `clack`'s entry-symbol macro should emit `clap_entry`
from either; smoke-build before committing wrapper CMake.

The wrapper is invoked from `xtask`. Nothing enters the Cargo
graph. CMake ≥ 3.21 becomes a build prerequisite for the VST3
path; the CLAP path is unaffected.

## In scope

- `vxn-clap` crate-type extension: `cdylib + rlib + staticlib`.
  Verify `clap_entry` symbol exports from the staticlib via a
  smoke link.
- Two new git submodules: `vendor/clap-wrapper` (pinned tag)
  and `vendor/vst3sdk` at 3.8.x (MIT). Override
  clap-wrapper's bundled SDK path via CMake var if its pinned
  tag still ships < 3.8.
- New `vxn-1/wrapper/CMakeLists.txt` driving clap-wrapper in
  bundled mode. Inputs: static archive path(s), VST3 SDK path,
  `CLAP_WRAPPER_OUTPUT_NAME=VXN1`. macOS universal slices
  combined via `CMAKE_OSX_ARCHITECTURES="arm64;x86_64"`.
- `xtask bundle` extension: `--format` flag accepting comma-
  separated `clap`, `vst3` (default `clap` to preserve current
  behaviour). VST3 path builds the staticlib slice(s), invokes
  CMake, copies `VXN1.vst3` to `target/bundled/`. `--install`
  routes to the platform's VST3 directory.
- Windows VST3 path in `xtask` — same CMake invocation, MSVC
  toolchain assumed in `vcvars64.bat` env. Document in README.
- Validation matrix: Reaper + Bitwig on mac, Cubase + Reaper +
  Live on Windows. Param automation round-trip, state save/
  load, HTML faceplate open + resize + multi-instance.
- CI: existing mac runner gains `--format clap,vst3 --release
  --universal` step. New `windows-latest` runner builds CLAP +
  VST3 x86_64. Artifacts uploaded.
- README install + build notes for VST3.

## Out of scope

- AUv2 / AUv3 (follow-up — same wrapper, separate ADR/epic
  once VST3 is stable).
- ~~Standalone format (clap-wrapper supports; no demand).~~
  **Superseded 2026-06-13** — standalone is now its own epic, vxn-2
  E010 (covering vxn-1 + vxn-2). Shares this epic's wrapper/CMake
  scaffold and `staticlib` link mode.
- Linux VST3 (trivial follow-up; same CMake).
- VST3 GUI features beyond what the CLAP `gui` extension
  exposes. Wrapper translates verbatim.
- Code signing / notarization beyond what the CLAP build
  already does — document as a separate `xtask sign` task in
  a future ticket; not required for plugin load.
- Migrating away from `clack` — the CLAP shell stays as-is.

## Phasing

- **0108** vxn-clap staticlib + entry-symbol smoke.
- **0109** Vendor submodules: clap-wrapper + vst3sdk 3.8.
- **0110** Wrapper CMakeLists (bundled, mac + win).
- **0111** xtask `--format vst3` macOS path (universal).
- **0112** xtask `--format vst3` Windows path.
- **0113** DAW validation matrix (mac + win).
- **0114** CI: VST3 artifacts on mac + new Windows runner.

## Dependency order

```text
0108 (staticlib)        ──┐
0109 (submodules)       ──┤  prep, independent
                          ├── 0110 (wrapper CMake) ── 0111 (xtask mac) ──┐
                          │                                              ├── 0113 (validation) ── 0114 (CI)
                          └─────────────────────────  0112 (xtask win) ──┘
```

0108 + 0109 can land in parallel. 0110 needs both. 0111 / 0112
both depend on 0110 and can land independently per platform.
0113 (validation) gates 0114 (CI ship) — don't enable an
artifact pipeline until the artifact loads in real hosts.

## Acceptance

- `cargo xtask bundle --release --format clap,vst3 --universal`
  on macOS produces `target/bundled/VXN1.clap` and
  `target/bundled/VXN1.vst3`. Both load in Reaper, both pass
  parameter automation round-trip, both save/restore state via
  a project file.
- The same command on Windows (sans `--universal`) produces
  both artifacts; VST3 loads in Cubase, Reaper, Live.
- HTML faceplate opens, resizes, and works in every validated
  host; second plugin instance is independent of the first.
- `cargo xtask bundle --format clap` is unchanged in behaviour
  and output — no regression for the CLAP-only path.
- CI publishes `VXN1.clap` + `VXN1.vst3` artifacts on mac
  (universal) and `VXN1.vst3` (x86_64) on Windows for every
  green build.
- README has install instructions for VST3 on both platforms.
- License audit: the shipping VST3 binary's transitive sources
  are MIT / Apache-2.0 / MIT-equivalent (vst3sdk 3.8 = MIT,
  clap-wrapper = MIT, clack = MIT-or-Apache).
