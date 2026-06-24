---
id: "0011"
product: vxn-1
title: xtask --format vst3 (macOS universal)
priority: high
created: 2026-06-08
epic: E010
---

## Summary

Extend `cargo xtask bundle` with a `--format` flag accepting
comma-separated values from `clap`, `vst3`. The `clap` path is
unchanged. The `vst3` path on macOS builds the `vxn-clap`
staticlib slice(s), invokes the wrapper CMake from ticket
0010, and copies the resulting `VXN1.vst3` to
`target/bundled/`. `--install` installs to
`~/Library/Audio/Plug-Ins/VST3/`. `--universal` produces a
universal `.vst3` by passing both arch static archives to
CMake and setting `CMAKE_OSX_ARCHITECTURES`.

Per ADR 0008 §4.

## Acceptance criteria

- [ ] `xtask/src/main.rs` parses `--format` as comma-separated
      values. Default when the flag is absent: `clap`. Unknown
      formats error with a clear message.
- [ ] Format dispatch:
      - `clap` runs the existing bundle path verbatim.
      - `vst3` runs the new VST3 path (this ticket).
      - Both can be passed together; their outputs land
        independently in `target/bundled/`.
- [ ] VST3 path on macOS:
      1. Build `vxn-clap` staticlib for the current host (or
         both `aarch64-apple-darwin` + `x86_64-apple-darwin`
         under `--universal`).
      2. Resolve archive path(s) via the same `lib_path`-
         style helper used for the dylib, swapped to `.a`.
      3. Invoke CMake with `VXN_CLAP_STATIC` (semicolon-
         joined archive list), `VXN_VST3_SDK_DIR`,
         `VXN_CLAP_WRAPPER_DIR` pointing at the submodule
         checkouts (resolved from `workspace_root()`),
         `VXN_OUTPUT_DIR=target/wrapper-{profile}/out`, and
         `CMAKE_OSX_ARCHITECTURES="arm64;x86_64"` under
         `--universal`. Generator: Ninja if available, else
         the platform default.
      4. Run `cmake --build target/wrapper-{profile}
         --parallel --config Release` (config flag for multi-
         config generators only; harmless on Ninja).
      5. Copy the produced `VXN1.vst3` bundle directory to
         `target/bundled/VXN1.vst3` (recursive copy, mirroring
         the existing `copy_dir_recursive`).
- [ ] `--install` for `vst3` copies the bundle to
      `~/Library/Audio/Plug-Ins/VST3/VXN1.vst3` (mirroring the
      CLAP install path logic — same `copy_clap` recursive
      copy helper applies to the bundle directory).
- [ ] Submodules check: if `vendor/clap-wrapper` or
      `vendor/vst3sdk` is missing or empty, error with a
      pointer to `git submodule update --init --recursive`
      rather than letting CMake fail opaquely.
- [ ] CMake check: if `cmake` is not on `PATH`, error with a
      clear install hint (homebrew / installer link).
- [ ] `cargo xtask bundle --release --format clap,vst3
      --universal --install` on macOS produces and installs
      both artifacts; both are loadable in Reaper (validation
      proper lives in ticket 0013).
- [ ] `cargo xtask bundle [--release]` with no `--format`
      flag is bit-identical in behaviour to before this
      ticket.

## Notes

The wrapper-build dir under `target/` should be reusable
across invocations — let CMake decide what to rebuild rather
than wiping the directory on each xtask run. If a clean
rebuild is needed, the user can `rm -rf target/wrapper-*`.

The universal-slice flow already exists for CLAP
(`build_universal`); refactor or duplicate the helper to take
a "what to build" callback so the VST3 path doesn't re-implement
the per-triple cargo loop. Duplication is fine for the first
pass — refactor only if the result is uglier.

The wrapper's CMake output layout may place `VXN1.vst3` under
a per-config subdirectory (`Release/`) on multi-config
generators. xtask should find the bundle by name rather than
hard-coding a relative path — glob `**/VXN1.vst3` under the
CMake build dir and pick the latest mtime.

Don't try to handle code signing here; out-of-scope per ADR
0008 "Out of scope".

If `cmake --build` fails, exit with a non-zero status and the
CMake error verbatim. Don't try to translate the error — the
underlying tooling is opaque to xtask and we'd lose detail.
