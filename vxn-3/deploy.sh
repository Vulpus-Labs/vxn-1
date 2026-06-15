#!/usr/bin/env bash
#
# Build VXN3, bundle it as vxn3.clap, and install it to the user CLAP directory.
#
# Delegates to `cargo xtask install`, which builds the release dylib, assembles
# the macOS .clap bundle (Contents/MacOS/vxn3 + Info.plist + PkgInfo), and copies
# it to ~/Library/Audio/Plug-Ins/CLAP/vxn3.clap.
#
# macOS only. Linux/Windows support is a follow-up.
#
# Usage:
#   ./deploy.sh
#
# After deploying, rescan plugins in your DAW (or restart it) to pick up VXN3.

set -euo pipefail

cd "$(dirname "$0")"

if [[ $# -gt 0 ]]; then
    echo "deploy.sh: unexpected argument '$1'" >&2
    exit 2
fi

echo "==> Building and installing vxn3.clap..."
cargo xtask install

echo "==> Done."
