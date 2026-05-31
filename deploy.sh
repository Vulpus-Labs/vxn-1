#!/usr/bin/env bash
#
# Build VXN1, bundle it as VXN1.clap, and install it to the user CLAP directory.
#
# This delegates to `cargo xtask bundle`, which knows how to assemble a proper
# macOS .clap *bundle* (Contents/MacOS/VXN1 + Info.plist) — a plain rename of the
# .dylib is not a valid plugin on macOS. On Linux/Windows the .clap is just the
# shared library renamed, which xtask also handles.
#
# Usage:
#   ./deploy.sh                       # release build, vizia editor, install
#   ./deploy.sh --debug               # debug build instead of release
#   ./deploy.sh --webview             # swap to the wry-backed editor (E010)
#   ./deploy.sh --debug --webview     # both flags compose
#
# `--webview` passes through to xtask, which builds vxn-clap with
# `--no-default-features --features webview`. Both bundles install to the
# same .clap path; the host loads whichever was built last.
#
# Install destinations (per OS, chosen by xtask):
#   macOS    ~/Library/Audio/Plug-Ins/CLAP/VXN1.clap
#   Linux    ~/.clap/VXN1.clap
#   Windows  %LOCALAPPDATA%\Programs\Common\CLAP\VXN1.clap

set -euo pipefail

# Run from the repository root (the directory containing this script).
cd "$(dirname "$0")"

PROFILE="--release"
WEBVIEW=""
for arg in "$@"; do
    case "$arg" in
        --debug)   PROFILE="" ;;
        --webview) WEBVIEW="--webview" ;;
        *) echo "deploy.sh: unknown flag '$arg'" >&2; exit 2 ;;
    esac
done

echo "==> Building and installing VXN1.clap${WEBVIEW:+ (webview)}..."
cargo xtask bundle ${PROFILE} --install ${WEBVIEW}

echo "==> Done."
