#!/usr/bin/env bash
#
# Build VXN4, bundle it as vxn4.clap, and install it to the user CLAP directory.
#
# Delegates to `cargo xtask`, which builds the release dylib, assembles the
# macOS .clap bundle (Contents/MacOS/vxn4 + Info.plist + PkgInfo), code-signs
# it, and copies it to ~/Library/Audio/Plug-Ins/CLAP/vxn4.clap. A plain rename
# of the .dylib is not a valid plugin on macOS, and an *unsigned* bundle is
# rejected by validating hosts and reported as "not a plugin" — which is what
# shipped broken in 0.3.0, so the signing is not optional politeness.
#
# CLAP only: vxn-4 has no wrapper CMake project, so there is no `--vst3` here
# the way vxn-1b has one. macOS only; Linux/Windows are a follow-up.
#
# vxn-4 has no faceplate, so unlike vxn-1b and vxn-2 there is no web build and
# no asset staging — the whole plugin is the dylib.
#
# Usage:
#   ./deploy.sh                # release build, bundle + install
#   ./deploy.sh --bundle-only  # build + assemble into target/bundled/, do not install
#   ./deploy.sh --uninstall    # remove the installed bundle
#
# Install destination (macOS):
#   ~/Library/Audio/Plug-Ins/CLAP/vxn4.clap
#
# After deploying, rescan plugins in your DAW (or restart it) to pick up VXN4.

set -euo pipefail

# Run from this script's directory (vxn-4/). Two reasons, not one: cargo walks
# up to the flat workspace root either way, but the `cargo xtask` alias lives in
# vxn-4/.cargo/config.toml and is only visible from here.
cd "$(dirname "$0")"

SUBCOMMAND="install"
for arg in "$@"; do
    case "$arg" in
        --bundle-only) SUBCOMMAND="bundle" ;;
        --uninstall)   SUBCOMMAND="uninstall" ;;
        *) echo "deploy.sh: unknown flag '$arg'" >&2; exit 2 ;;
    esac
done

echo "==> VXN4: cargo xtask ${SUBCOMMAND}"
cargo xtask "${SUBCOMMAND}"

echo "==> Done."
