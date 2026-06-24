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

## Close-out (2026-06-24)

Implemented host-agnostically; the macOS path (0011) and the new Windows path
share one `bundle_vst3`. Verified buildable on the macOS dev host
(`cargo build`/`clippy -p vxn1-xtask` clean); **Windows runtime build + DAW
load is deferred to the validation matrix (0013)** — no Windows box here, same
as 0011 deferred Reaper/Bitwig load.

- Platform guard: [main.rs:217](../../vxn-1/xtask/src/main.rs#L219)
  `bundle_vst3` now accepts macOS **or** Windows; errors
  `--format vst3 is supported on macOS and Windows only` elsewhere (Linux is a
  trivial follow-up per the epic). `--universal` still rejects off macOS with
  `--universal is macOS-only (omit it on Windows; the build is x86_64)`.
- MSVC preflight: [main.rs:ensure_msvc](../../vxn-1/xtask/src/main.rs#L422)
  spawns `cl.exe`; if it's not on PATH, errors with the
  "Developer PowerShell for VS 2022 / vcvars64.bat" hint. No-op on non-Windows.
  We do **not** locate or source vcvars ourselves (the rabbit hole the ticket
  warns against) — the Developer shell also supplies the `INCLUDE`/`LIB` env the
  Ninja+MSVC build inherits.
- Static archive: `static_lib_path` already emits `vxn_clap.lib` (no `lib`
  prefix, `.lib` ext) on Windows; the non-universal cargo build produces it
  alongside the cdylib (`vxn_clap.dll` + its `.dll.lib` import lib — no clash).
  Passed to CMake via `-DVXN_CLAP_STATIC`.
- Generator: the existing `ninja_available()` gate adds `-G Ninja` when present,
  else the platform default (VS multi-config); `cmake --build … --parallel
  --config Release` is correct for both. The wrapper CMake (0010) already
  whole-archives via `/WHOLEARCHIVE:` on WIN32, links the win32 system libs
  (`ws2_32 userenv ntdll bcrypt user32 …`), and stages the folder bundle through
  the non-APPLE `copy_directory` branch.
- Install: [main.rs:vst3_install_dir](../../vxn-1/xtask/src/main.rs#L501)
  Windows branch now resolves `%LOCALAPPDATA%\Programs\Common\VST3` (per-user, no
  admin) instead of the machine-wide `%CommonProgramFiles%\VST3`; bundle copied
  recursively via the same `copy_clap`/`copy_dir_recursive` used on macOS.
- Regression: CLAP-only `bundle [--release]` on Windows is untouched — copies
  `vxn_clap.dll` → `VXN1.clap` exactly as before.

Acceptance boxes left unchecked pending the Windows host pass in 0013, mirroring
0011's "load proper lives in 0013" deferral.
