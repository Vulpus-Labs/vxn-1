#!/usr/bin/env bash
#
# Build VXN1b, bundle it as vxn1b.clap (and optionally VXN1b.vst3), and install
# it to the user plug-in directories.
#
# Delegates to `vxn1b-xtask`, which assembles a proper macOS .clap *bundle*
# (Contents/MacOS/vxn1b + Info.plist + PkgInfo) — a plain rename of the .dylib
# is not a valid plugin on macOS. The faceplate assets are `include_str!`-
# embedded in the cdylib, so the bundle is just the dylib + plist + PkgInfo.
#
# Unlike vxn-1/vxn-2, VXN1b has no `cargo xtask` alias (no per-product
# .cargo/config.toml), so this calls the xtask package directly. The build is
# always release; cargo's own freshness check makes a no-change rebuild a no-op.
#
# Artifacts are staged in `target/bundled/` (0213 moved them there from
# `target/release/`, matching vxn-1 and vxn-2 — a universal build's output does
# not belong in a host-target profile dir).
#
# The VST3 path (`--vst3`) wraps the same code through clap-wrapper; it needs
# CMake and the repo-root submodules (`git submodule update --init --recursive`).
# macOS and Windows only. Linux bundling is CLAP-only.
#
# Usage:
#   ./deploy.sh                # release build, bundle + install the CLAP
#   ./deploy.sh --bundle-only  # build + assemble, do not install
#   ./deploy.sh --uninstall    # remove the installed artifact(s)
#   ./deploy.sh --vst3         # also build/install VXN1b.vst3
#   ./deploy.sh --universal    # macOS: arm64 + x86_64 in one fat binary
#
# Install destinations (macOS):
#   ~/Library/Audio/Plug-Ins/CLAP/vxn1b.clap
#   ~/Library/Audio/Plug-Ins/VST3/VXN1b.vst3   (with --vst3)

set -euo pipefail

# Run from this script's directory (vxn-1b/); cargo walks up to the flat
# workspace root either way, but this keeps relative output predictable.
cd "$(dirname "$0")"

SUBCOMMAND="install"
FORMAT="clap"
EXTRA=()
for arg in "$@"; do
    case "$arg" in
        --bundle-only) SUBCOMMAND="bundle" ;;
        --uninstall)   SUBCOMMAND="uninstall" ;;
        --vst3)        FORMAT="clap,vst3" ;;
        --universal)   EXTRA+=("--universal") ;;
        *) echo "deploy.sh: unknown flag '$arg'" >&2; exit 2 ;;
    esac
done

echo "==> VXN1b: cargo run -p vxn1b-xtask -- ${SUBCOMMAND} --format ${FORMAT} ${EXTRA[*]-}"
cargo run --quiet --package vxn1b-xtask -- \
    "${SUBCOMMAND}" --format "${FORMAT}" ${EXTRA[@]+"${EXTRA[@]}"}

echo "==> Done."
