#!/usr/bin/env bash
# Deploy the VXN-1b web bundle to the vulpus-labs-site Hugo repo (ticket 0294).
#
# Mirrors vxn-2/crates/vxn2-wasm/deploy-web.sh: build the bundle, copy it into
# the site's static tree at the synth subpath, ensure the root _headers carries
# THIS subpath's cross-origin isolation block (COOP/COEP — required for
# SharedArrayBuffer), then stage, commit and push. The site auto-deploys from
# `main` via Netlify, so the push IS the deploy.
#
#   ./deploy-web.sh                      # build + copy + commit + push
#   SITE=~/elsewhere ./deploy-web.sh     # point at a different site checkout
#   NO_PUSH=1 ./deploy-web.sh            # build + copy + commit, but don't push
#   NO_BUILD=1 ./deploy-web.sh           # reuse the existing bundle
#
# The _headers block is APPENDED IF MISSING, never rewritten: that file carries
# one block per synth, and there are now three. Overwriting it takes the other
# two synths off the air — their pages still load and then fail to construct a
# SharedArrayBuffer, which reads as "the synth is broken", not as "a header is
# missing". vxn-1's script used to do exactly that; see its header.
set -euo pipefail

# --- config (override via env) ---
SITE="${SITE:-$HOME/src/vulpus-labs-site}"
SUBPATH="${SUBPATH:-products/vxn-1b/web}" # → https://vulpuslabs.com/products/vxn-1b/web/
BRANCH="${BRANCH:-main}"

# Workspace root = three levels up (vxn-1b/crates/vxn1b-wasm/ → monorepo root),
# where target/ and the cargo workspace live.
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../../.." && pwd)"
# VXN1b's bundle has its own directory: the three ports build concurrently and a
# shared target/web-dist would have them overwrite each other.
DIST="$ROOT/target/web-dist-vxn1b"

[ -d "$SITE/.git" ] || { echo "error: $SITE is not a git checkout" >&2; exit 1; }

# 1. Build the bundle (unless reusing an existing one). VXN1b has no `cargo
#    xtask` alias — no per-product .cargo/config.toml — so call the package.
if [ -z "${NO_BUILD:-}" ]; then
  echo "==> building web bundle"
  ( cd "$ROOT" && cargo run --quiet -p vxn1b-xtask -- web )
fi
[ -f "$DIST/index.html" ] || { echo "error: no bundle at $DIST (run without NO_BUILD)" >&2; exit 1; }

# 2. Copy the bundle into the site's static tree. --delete keeps it a clean
#    mirror (stale files from an old build are removed). The bundle's own
#    _headers is excluded — Netlify only reads _headers at the deploy root.
DEST="$SITE/static/$SUBPATH"
echo "==> copying bundle → static/$SUBPATH/"
mkdir -p "$DEST"
rsync -a --delete --exclude _headers "$DIST"/ "$DEST"/

# 3. Ensure the root _headers has this subpath's isolation block. Scoped to the
#    subpath so the rest of the site is untouched (COOP/COEP site-wide can break
#    third-party embeds / popups).
HEADERS="$SITE/static/_headers"
if ! grep -qF "/$SUBPATH/*" "$HEADERS" 2>/dev/null; then
  echo "==> appending static/_headers block for /$SUBPATH/"
  cat >> "$HEADERS" <<EOF

# Same cross-origin isolation for the VXN-1b web synth.
/$SUBPATH/*
  Cross-Origin-Opener-Policy: same-origin
  Cross-Origin-Embedder-Policy: require-corp
  Cross-Origin-Resource-Policy: same-origin
EOF
else
  echo "==> static/_headers already covers /$SUBPATH/"
fi

# 4. Stage, commit, push. Only our paths — the site checkout may carry unrelated
#    work in progress.
cd "$SITE"
git add "static/$SUBPATH" static/_headers
if git diff --cached --quiet; then
  echo "==> no changes to commit (site already up to date)"
  exit 0
fi
VER="$(cd "$ROOT" && git rev-parse --short HEAD)"
git commit -m "deploy: VXN-1b web synth → /$SUBPATH/ (vxn-1b @ $VER)" \
  -m "Built from cargo run -p vxn1b-xtask -- web; _headers sets COOP/COEP for SharedArrayBuffer."

if [ -n "${NO_PUSH:-}" ]; then
  echo "==> committed (NO_PUSH set — not pushing). Push manually to deploy."
  exit 0
fi
echo "==> pushing to $BRANCH (Netlify will deploy)"
git push origin "$BRANCH"
echo "==> done. Verify once live:"
echo "    curl -sI https://vulpuslabs.com/$SUBPATH/ | grep -i cross-origin"
