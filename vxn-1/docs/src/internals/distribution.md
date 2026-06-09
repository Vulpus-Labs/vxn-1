# CLAP & VST3 distribution

VXN1 currently ships **CLAP** only. **VST3 via [`clap-wrapper`](https://github.com/free-audio/clap-wrapper)** is the committed distribution path (ADR 0008), but the wrapper integration has not yet landed in `xtask` and there is no `vendor/clap-wrapper` submodule in the workspace. Treat the VST3 section below as the target state, not the current build.

## CLAP build

`vxn-clap` is a `cdylib` linking `vxn-engine`, `vxn-app`, and `vxn-ui-web`. Built via `cargo build --release`, the resulting `libvxn_clap.{dylib,so,dll}` is packaged into a `VXN1.clap` bundle by `xtask`.

Bundle structure (macOS):

```
VXN1.clap/
└── Contents/
    ├── Info.plist
    ├── MacOS/
    │   └── VXN1
    └── PkgInfo
```

Windows and Linux ship the cdylib as a flat `VXN1.clap` file with no bundle wrapper.

## VST3 build (planned, not yet shipping)

The committed plan (ADR 0008) is for `xtask` to invoke a vendored `clap-wrapper` build via CMake, with `vendor/clap-wrapper` and `vendor/vst3sdk` (pinned to VST3 3.8 — MIT-licensed since October 2025) as workspace submodules. The build will produce a **single-binary bundled VST3** with the CLAP cdylib statically linked.

Planned requirements:

- CMake ≥ 3.21.
- A C++17 compiler (clang, MSVC, GCC).

Planned xtask invocations (none of these flags are accepted today):

```sh
# VST3 only (planned)
cargo xtask bundle --release --format vst3

# Both formats in one shot (planned)
cargo xtask bundle --release --format clap,vst3

# macOS universal binary (planned)
cargo xtask bundle --release --universal --format vst3
```

For the actual shipping CLAP build, see [Installing VXN1](../install.md).

## Install locations

| OS | CLAP (shipping) | VST3 (planned) |
| --- | --- | --- |
| **macOS** | `~/Library/Audio/Plug-Ins/CLAP/VXN1.clap` | `~/Library/Audio/Plug-Ins/VST3/VXN1.vst3` |
| **Windows** | `%LOCALAPPDATA%\Programs\Common\CLAP\VXN1.clap` | `%LOCALAPPDATA%\Programs\Common\VST3\VXN1.vst3` |
| **Linux** | `~/.clap/VXN1.clap` | _not planned for the first VST3 cut_ |

Bundle identifier: `labs.vulpus.vxn1`.

## Parameter identity

VST3 parameter IDs are derived from CLAP parameter IDs by hashing. The implication:

> Renaming a CLAP parameter ID breaks VST3 automation in saved projects.

ADR 0008 commits to a **soft stability policy**: CLAP IDs won't be renamed post-ship, except when called out in release notes. Pre-release, IDs are free to change.

If you're writing host-side integrations against the VST3 binary, you can rely on stable hashed IDs from release to release.

## Code signing

Pre-release builds are **not signed**. See [Unsigned binaries](../install-unsigned.md) for the macOS Gatekeeper workaround and Windows SmartScreen handling.

The release pipeline plan is:

- **macOS**: codesign with a Developer ID Application certificate + notarisation via `xcrun notarytool`.
- **Windows**: Authenticode signing of the `.clap` and `.vst3`.
- **Linux**: no signing; SHA256SUMS in release artefacts.

Until release, expect to clear Gatekeeper quarantine by hand or build from source.

## Plugin discovery

CLAP and VST3 both rely on filesystem scanning by the host. If VXN1 doesn't appear after install:

1. Verify the install path matches the OS table above.
2. Confirm the host's plugin search paths include the standard CLAP / VST3 directories (most hosts do this by default, but some Linux distros and locked-down corporate machines need explicit configuration).
3. On macOS, check the quarantine flag is cleared (see [Unsigned binaries](../install-unsigned.md)).
4. Force a plugin rescan in the host. Some hosts cache failure metadata and won't retry until told to.
