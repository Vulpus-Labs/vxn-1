#!/usr/bin/env bash
# Deploy the VXN1 web bundle to the vulpus-labs-site Hugo repo (ticket 0045).
#
# Builds target/web-dist/ (cargo xtask web), copies it into the site's static
# tree at the synth subpath, refreshes the root _headers that turns on
# cross-origin isolation (COOP/COEP — required for SharedArrayBuffer), then
# stages, commits, and pushes. The site auto-deploys from `main` via Netlify, so
# the push IS the deploy.
#
#   ./deploy-web.sh                      # build + copy + commit + push
#   SITE=~/elsewhere ./deploy-web.sh     # point at a different site checkout
#   NO_PUSH=1 ./deploy-web.sh            # build + copy + commit, but don't push
#   NO_BUILD=1 ./deploy-web.sh           # reuse the existing target/web-dist
set -euo pipefail

# --- config (override via env) ---
SITE="${SITE:-$HOME/src/vulpus-labs-site}"
SUBPATH="${SUBPATH:-products/vxn-1/web}"   # → https://vulpuslabs.com/products/vxn-1/web/
BRANCH="${BRANCH:-main}"

# Workspace root = three levels up from this script (crates/vxn-wasm/ → root),
# where target/ and the cargo workspace live.
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../../.." && pwd)"
DIST="$ROOT/target/web-dist"

[ -d "$SITE/.git" ] || { echo "error: $SITE is not a git checkout" >&2; exit 1; }

# 1. Build the bundle (unless reusing an existing one).
if [ -z "${NO_BUILD:-}" ]; then
  echo "==> building web bundle"
  ( cd "$ROOT" && cargo run -q -p vxn1-xtask -- web )
fi
[ -f "$DIST/index.html" ] || { echo "error: no bundle at $DIST (run without NO_BUILD)" >&2; exit 1; }

# 2. Copy the bundle into the site's static tree. --delete keeps it a clean
#    mirror (stale files from an old build are removed). The bundle's own
#    _headers is excluded — Netlify only reads _headers at the deploy root.
DEST="$SITE/static/$SUBPATH"
echo "==> copying bundle → static/$SUBPATH/"
mkdir -p "$DEST"
rsync -a --delete --exclude _headers "$DIST"/ "$DEST"/

# 3. Refresh the root _headers (cross-origin isolation, scoped to the subpath so
#    the rest of the site is untouched). Idempotent — rewritten each run.
echo "==> writing static/_headers"
cat > "$SITE/static/_headers" <<EOF
# Cross-origin isolation for the VXN1 web synth (vxn-1 ticket 0045).
# SharedArrayBuffer (its audio transport) needs the page cross-origin isolated,
# which needs COOP: same-origin + COEP: require-corp on the document. Scoped to
# the synth subpath so the rest of the site is unaffected (COOP/COEP site-wide
# can break third-party embeds / popups).
/$SUBPATH/*
  Cross-Origin-Opener-Policy: same-origin
  Cross-Origin-Embedder-Policy: require-corp
  Cross-Origin-Resource-Policy: same-origin
EOF

# 4. Stage, commit, push.
cd "$SITE"
git add "static/$SUBPATH" static/_headers
if git diff --cached --quiet; then
  echo "==> no changes to commit (site already up to date)"
  exit 0
fi
WASM_VER="$(cd "$ROOT" && git rev-parse --short HEAD)"
git commit -m "deploy: VXN1 web synth → /$SUBPATH/ (vxn-1 @ $WASM_VER)" \
  -m "Built from cargo xtask web; _headers sets COOP/COEP for SharedArrayBuffer." \
  -m "Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"

if [ -n "${NO_PUSH:-}" ]; then
  echo "==> committed (NO_PUSH set — not pushing). Push manually to deploy."
  exit 0
fi
echo "==> pushing to $BRANCH (Netlify will deploy)"
git push origin "$BRANCH"
echo "==> done. Verify once live:"
echo "    curl -sI https://vulpuslabs.com/$SUBPATH/ | grep -i cross-origin"
