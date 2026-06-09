# Installing VXN1

VXN1 currently ships as a **CLAP** plugin (canonical format, statically linked, no external runtime dependencies). VST3 distribution via `clap-wrapper` is planned (see ADR 0008 and [Distribution](internals/distribution.md)) but not yet wired into the build.

## Prerequisites

- Rust **1.85+** (edition 2024).
- macOS, Windows (x86_64), or Linux. Apple Silicon is the primary target; universal builds (arm64 + x86_64) are supported on macOS.

## Build & install from source

From the workspace root (`vxn-1/`):

```sh
# CLAP, install to user directory
cargo xtask bundle --release --install

# CLAP, no install (bundle stays in target/)
cargo xtask bundle --release

# macOS universal (arm64 + x86_64), installed
cargo xtask bundle --release --universal --install
```

The available xtask flags are `--release`, `--install`, and `--universal`. Without `--install`, the `.clap` bundle is written under `target/` and can be copied by hand.

## Install locations

| OS | CLAP |
| --- | --- |
| **macOS** | `~/Library/Audio/Plug-Ins/CLAP/VXN1.clap` |
| **Windows** | `%LOCALAPPDATA%\Programs\Common\CLAP\VXN1.clap` |
| **Linux** | `~/.clap/VXN1.clap` |

Bundle identifier: `labs.vulpus.vxn1`.

## Verifying the install

1. Restart your DAW (or rescan plugins).
2. Look for **VXN1** under instruments / Vulpus Labs.
3. Load it on a MIDI track. The faceplate should show the default patch (Saw / Saw, mid-cutoff, chorus on).

If the plugin doesn't appear:

- Check the install path matches the DAW's CLAP search paths.
- On macOS, see [Unsigned binaries](install-unsigned.md) for Gatekeeper quarantine.
- Confirm the host supports CLAP. Most modern hosts (Bitwig, Reaper, recent FL Studio / Ableton) do; some still need a CLAP plugin to be enabled in settings.
