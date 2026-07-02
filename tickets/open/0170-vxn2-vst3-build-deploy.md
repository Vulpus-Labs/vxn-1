---
id: "0170"
product: vxn-2
title: "vxn-2 VST3 build + deploy path (clap-wrapper VST3 target + xtask vst3 subcommand)"
priority: medium
created: 2026-07-02
epic: null
depends: []
---

## Summary

vxn-2 ships only a CLAP plugin (+ standalone app). There is no VST3 build
path: [xtask/src/main.rs](../../vxn-2/xtask/src/main.rs) exposes only
`bundle` / `install` / `standalone`, and the repo has no VST3 wrapper CMake
for `vxn2-clap` — `standalone/CMakeLists.txt` only invokes
`target_add_standalone_wrapper`. So `deploy.sh` cannot deploy a VST3, and
users on VST3-only hosts get nothing.

vxn-1 already solved this (E010): its
[wrapper/CMakeLists.txt](../../vxn-1/wrapper/CMakeLists.txt) wraps the
`vxn-clap` staticlib into `VXN1.vst3` via clap-wrapper, and
[xtask/src/main.rs](../../vxn-1/xtask/src/main.rs) exposes
`bundle --format clap,vst3 --install` (`bundle_vst3`). Port that approach to
vxn-2. The `vxn2-clap` crate already builds a `staticlib`
([crates/vxn2-clap/Cargo.toml:14](../../vxn-2/crates/vxn2-clap/Cargo.toml#L14)),
so the archive side is ready.

## Design

- Add a VST3 wrapper CMake target for `vxn2-clap` — either extend the shared
  `standalone/CMakeLists.txt` (it already whole-archives the vxn2 staticlib)
  with a `target_add_vst3_wrapper(...)` target, or author a
  `vxn-2/wrapper/CMakeLists.txt` mirroring vxn-1's. Prefer reusing the
  vendored clap-wrapper submodule — do not vendor a second copy.
- Add an xtask `vst3` build path: a `--format clap,vst3` flag on `bundle`
  (mirror vxn-1) or a dedicated `vst3` subcommand, plus `--install` to the
  user VST3 dir (`~/Library/Audio/Plug-Ins/VST3/VXN2.vst3` on macOS,
  `%LOCALAPPDATA%\Programs\Common\VST3\VXN2.vst3` on Windows). macOS
  (universal) + Windows (x86_64 MSVC) only, matching vxn-1.
- Wire `vxn-2/deploy.sh` to request VST3 by default, with a `--clap-only`
  escape hatch (mirror the vxn-1 deploy.sh just updated).

## Acceptance criteria

- [ ] `cargo xtask` (vxn-2) builds `target/bundled/VXN2.vst3` and `--install`
      copies it to the user VST3 directory on macOS and Windows.
- [ ] The staged `VXN2.vst3` loads and passes the host's plugin scan (e.g.
      validator / DAW rescan) on macOS.
- [ ] `vxn-2/deploy.sh` installs both `VXN2.clap` and `VXN2.vst3` by default;
      `--clap-only` skips the VST3.
- [ ] No second copy of clap-wrapper vendored; VST3 target reuses the
      existing submodule.

## Notes

- Related: vxn-1 E010 (VST3) and E014 0027 (vxn-2 clap-wrapper standalone
  scaffold) — the wrapper submodule + staticlib groundwork already exist.
- Out of scope: Linux VST3 (clap-wrapper VST3 is macOS + Windows only here).
- Follow-up to the deploy.sh update that added `--format clap,vst3` to
  vxn-1's deploy.sh; vxn-2 deferred pending this build path.
