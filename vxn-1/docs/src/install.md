# Installing VXN1

VXN1 ships as a single binary in two plugin formats:

- **CLAP** — canonical format, statically linked, no external runtime dependencies.
- **VST3** — produced by wrapping the CLAP via [`clap-wrapper`](https://github.com/free-audio/clap-wrapper). Single-binary bundled package; no separate `.clap` dependency at runtime.

## Prerequisites

- Rust **1.85+** (edition 2024).
- For VST3 builds: **CMake ≥ 3.21**.
- macOS, Windows (x86_64), or Linux. Apple Silicon is the primary target; universal builds (arm64 + x86_64) are supported on macOS.

## Build & install from source

From the workspace root (`vxn-1/vxn-1/`):

```sh
# CLAP only
cargo xtask bundle --release --install

# VST3 only
cargo xtask bundle --release --format vst3 --install

# Both
cargo xtask bundle --release --format clap,vst3 --install

# macOS universal (arm64 + x86_64)
cargo xtask bundle --release --universal --install
```

Without `--install`, bundles are written to `target/bundled/VXN1.clap` and `target/bundled/VXN1.vst3` and can be copied by hand.

## Install locations

| OS | CLAP | VST3 |
| --- | --- | --- |
| **macOS** | `~/Library/Audio/Plug-Ins/CLAP/VXN1.clap` | `~/Library/Audio/Plug-Ins/VST3/VXN1.vst3` |
| **Windows** | `%LOCALAPPDATA%\Programs\Common\CLAP\VXN1.clap` | `%LOCALAPPDATA%\Programs\Common\VST3\VXN1.vst3` |
| **Linux** | `~/.clap/VXN1.clap` | _not currently shipped_ |

Bundle identifier: `labs.vulpus.vxn1`.

## Verifying the install

1. Restart your DAW (or rescan plugins).
2. Look for **VXN1** under instruments / Vulpus Labs.
3. Load it on a MIDI track. The faceplate should show the default patch (Saw / Saw, mid-cutoff, chorus on).

If the plugin doesn't appear:

- Check the install path matches the DAW's plugin search paths.
- On macOS, see [Unsigned binaries](install-unsigned.md) for Gatekeeper quarantine.
- Confirm format support: VST3 requires the host to support VST3 3.7+; some legacy hosts are VST2-only.

## Distribution notes (ADR 0008)

CLAP is the canonical format. The VST3 build is *produced* from the same CLAP binary via `clap-wrapper`; there is no separate VST3 source tree. Parameter IDs in VST3 are derived from CLAP IDs by hashing — renaming a CLAP ID would break VST3 automation in saved projects, so identifier stability is a soft guarantee post-ship.
