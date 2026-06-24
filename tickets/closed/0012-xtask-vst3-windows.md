---
id: "0012"
product: vxn-1
title: xtask --format vst3 (Windows)
priority: medium
created: 2026-06-08
epic: E010
---

## Summary

Add the Windows path to `xtask bundle --format vst3`. Build
`vxn-clap` as a static archive (`vxn_clap.lib`) for the host
triple, invoke the wrapper CMake from ticket 0010 under MSVC
2022 + Ninja, and install the resulting `VXN1.vst3` bundle
directory to `%LOCALAPPDATA%\Programs\Common\VST3\`.

Per ADR 0008 §4.

## Acceptance criteria

- [ ] `cargo xtask bundle --release --format vst3` on
      Windows (host: `x86_64-pc-windows-msvc`) produces
      `target/bundled/VXN1.vst3/` populated as a VST3 bundle:
      `Contents/x86_64-win/VXN1.vst3` shared library +
      `desktop.ini` + `Plugin.ico` (whatever the wrapper
      emits by default).
- [ ] `--install` on Windows copies the bundle directory to
      `%LOCALAPPDATA%\Programs\Common\VST3\VXN1.vst3` via
      the same recursive copy helper used on macOS.
- [ ] CMake invocation uses Ninja (`-G Ninja`) when
      available, falling back to the platform default. Build
      runs `cmake --build target/wrapper-{profile}
      --config Release --parallel`.
- [ ] The static archive path passed via `VXN_CLAP_STATIC` is
      resolved through the platform-specific lib-path helper
      (no `lib` prefix, `.lib` extension on Windows).
- [ ] If `cl.exe` / MSVC env is not on `PATH`, xtask errors
      with a hint to run from a "Developer PowerShell for VS
      2022" or invoke `vcvars64.bat` first. We don't try to
      locate and source `vcvars` ourselves — that's a rabbit
      hole.
- [ ] `--universal` rejects on Windows with a clear message
      (macOS-only flag, mirroring current CLAP behaviour).
- [ ] Existing `cargo xtask bundle [--release]` (no
      `--format` flag) is unchanged on Windows — still builds
      and installs the `.clap`.

## Notes

The wrapper's Windows VST3 bundle layout differs from macOS
(folder-style bundle, not an `Info.plist`-driven `.app`-like
structure). xtask's recursive copy already handles both; just
treat the produced `VXN1.vst3` as a directory and copy it
recursively.

MSVC 2022 + Ninja is the chosen toolchain. Visual Studio's
multi-config generator would also work, but Ninja is faster
and produces a single-config build dir that's easier to
glob the artifact out of (see ticket 0011 notes).

PATH-resolved Ninja: `where ninja`. If missing, hint at
`winget install Ninja-build.Ninja` or shipping with Visual
Studio's "Desktop development with C++" workload (Ninja is
bundled).

ARM64 Windows is deliberately not in scope for the first cut
— vanishingly few DAWs ship native arm64 builds in 2026.
Add later when demand exists.

Code signing: out of scope. Unsigned `.vst3` loads in
DAWs in dev. Distribution-grade signing arrives in a future
ticket via signtool / EV cert workflow.
