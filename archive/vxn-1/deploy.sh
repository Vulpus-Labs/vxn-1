#!/usr/bin/env bash
#
# Build VXN1, bundle it as VXN1.clap + VXN1.vst3, and install both to the user
# plugin directories.
#
# This delegates to `cargo xtask bundle`, which knows how to assemble a proper
# macOS .clap *bundle* (Contents/MacOS/VXN1 + Info.plist) — a plain rename of the
# .dylib is not a valid plugin on macOS. On Linux/Windows the .clap is just the
# shared library renamed, which xtask also handles. `--format clap,vst3` also
# wraps the same code as a VST3 module via the clap-wrapper CMake project (VST3
# is macOS + Windows only).
#
# Usage:
#   ./deploy.sh                       # release build, install CLAP + VST3
#   ./deploy.sh --debug               # debug build instead of release
#   ./deploy.sh --clap-only           # skip the VST3 (e.g. on Linux)
#
# Install destinations (per OS, chosen by xtask):
#   macOS    ~/Library/Audio/Plug-Ins/CLAP/VXN1.clap
#            ~/Library/Audio/Plug-Ins/VST3/VXN1.vst3
#   Linux    ~/.clap/VXN1.clap                        (VST3 not supported)
#   Windows  %LOCALAPPDATA%\Programs\Common\CLAP\VXN1.clap
#            %LOCALAPPDATA%\Programs\Common\VST3\VXN1.vst3

set -euo pipefail

# Run from the repository root (the directory containing this script).
cd "$(dirname "$0")"

PROFILE="--release"
FORMAT="clap,vst3"
for arg in "$@"; do
    case "$arg" in
        --debug)      PROFILE="" ;;
        --clap-only)  FORMAT="clap" ;;
        *) echo "deploy.sh: unknown flag '$arg'" >&2; exit 2 ;;
    esac
done

echo "==> Building and installing VXN1 (${FORMAT})..."
cargo xtask bundle ${PROFILE} --install --format "${FORMAT}"

echo "==> Done."
