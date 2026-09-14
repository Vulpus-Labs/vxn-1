---
id: "0374"
product: monorepo
title: "cargo xtask bundle --format au — build, sign and install the .component"
priority: high
created: 2026-09-10
epic: E051
depends: ["0373"]
---

## Summary

Make AU a first-class bundle format in the shared xtask, so
`cargo xtask bundle --release --format clap,vst3,au --universal` is one command
per product. Eighth ticket of
[E051](../../epics/open/E051-audio-unit-distribution.md).

The scaffolding is all there: `Format` parsing
([vxn-xtask-common/src/lib.rs:124](../../crates/vxn-xtask-common/src/lib.rs#L124)),
per-format install dirs
([lib.rs:232](../../crates/vxn-xtask-common/src/lib.rs#L232)), the CMake drive
([lib.rs:809](../../crates/vxn-xtask-common/src/lib.rs#L809)), the universal
lipo, and `codesign_bundle`
([lib.rs:457](../../crates/vxn-xtask-common/src/lib.rs#L457)). AU is another
arm through the same machinery.

## Design

- `Format::Au`, parsed from `au` and named in the "expected one of" error.
- `Product::au: Option<Au>` mirroring `Product::vst3: Option<Vst3>`
  ([lib.rs:60](../../crates/vxn-xtask-common/src/lib.rs#L60)), carrying the
  bundle stem plus the codes from
  [0372](0372-adr-au-component-identity.md) — this is the single place they
  live, passed down as `-DAUV2_*` cache vars.
- `au_install_dir()` → `~/Library/Audio/Plug-Ins/Components`, macOS-only. The
  existing `user_plugin_dir` helper takes per-OS arms; AU has no Windows or
  Linux arm, so this is the first format that is legitimately macOS-only and
  the error must say so rather than pointing at a missing directory.
- Preflight: reuse `vst3_preflight`'s shape — CMake present, submodules
  checked out (now including AudioUnitSDK), macOS host — and fail with a reason
  the way `vst3_or_err` does for CLAP-only products
  ([lib.rs:236](../../crates/vxn-xtask-common/src/lib.rs#L236)).
- **Codesign the `.component`.** Non-negotiable and for the same reason as
  every other bundle: the linker's ad-hoc signature declares a sealed resource
  directory that does not exist, and a validating host rejects the bundle
  outright ([lib.rs:430-456](../../crates/vxn-xtask-common/src/lib.rs#L430-L456)).
  Must run after resource staging, as there.
- Per-format failure isolation stays as it is: a failed AU leg must not take
  the CLAP or VST3 legs down with it
  ([lib.rs:165](../../crates/vxn-xtask-common/src/lib.rs#L165)).

## Acceptance criteria

- [ ] `--format au` and `--format clap,vst3,au` both work for vxn-2 on macOS;
      order and dedup behaviour matches the existing parser's.
- [ ] `--format au` off macOS fails with "AU is macOS-only", not a CMake error.
- [ ] `--format au` for a product with `au: None` fails with the same shape of
      message `vst3_or_err` produces.
- [ ] The staged `VXN2.component` is ad-hoc signed and
      `codesign --verify --deep --strict` passes.
- [ ] `xtask install` puts it in `~/Library/Audio/Plug-Ins/Components` and
      Logic scans it.
- [ ] Existing xtask tests green; new unit test for `--format` parsing of `au`.

## Notes

The four-char codes reaching CMake from Rust means the ADR's table has exactly
one runtime representation. Resist the temptation to also hard-code them in the
wrapper CMake as defaults — a default here is a second source of truth for an
identifier that can never change.
