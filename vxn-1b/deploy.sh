#!/usr/bin/env bash
#
# Build VXN1b, bundle it as vxn1b.clap, and install it to the user CLAP
# directory.
#
# Delegates to `vxn1b-xtask`, which assembles a proper macOS .clap *bundle*
# (Contents/MacOS/vxn1b + Info.plist + PkgInfo) — a plain rename of the .dylib
# is not a valid plugin on macOS. The faceplate assets are `include_str!`-
# embedded in the cdylib, so the bundle is just the dylib + plist + PkgInfo.
#
# Unlike vxn-1/vxn-2, VXN1b has no `cargo xtask` alias (no per-product
# .cargo/config.toml), so this calls the xtask package directly. The build is
# always release; xtask's own freshness check makes a no-change rebuild a no-op.
#
# macOS only. Linux/Windows bundling and the VST3 wrapper path are follow-ups
# (see vxn-1b/xtask/src/main.rs).
#
# Usage:
#   ./deploy.sh                # release build, bundle + install the CLAP
#   ./deploy.sh --bundle-only  # build + assemble the bundle, do not install
#   ./deploy.sh --uninstall    # remove the installed bundle
#
# Install destination (macOS):
#   ~/Library/Audio/Plug-Ins/CLAP/vxn1b.clap

set -euo pipefail

# Run from this script's directory (vxn-1b/); cargo walks up to the flat
# workspace root either way, but this keeps relative output predictable.
cd "$(dirname "$0")"

SUBCOMMAND="install"
for arg in "$@"; do
    case "$arg" in
        --bundle-only) SUBCOMMAND="bundle" ;;
        --uninstall)   SUBCOMMAND="uninstall" ;;
        *) echo "deploy.sh: unknown flag '$arg'" >&2; exit 2 ;;
    esac
done

echo "==> VXN1b: cargo run -p vxn1b-xtask -- ${SUBCOMMAND}"
cargo run --quiet --package vxn1b-xtask -- "${SUBCOMMAND}"

echo "==> Done."
