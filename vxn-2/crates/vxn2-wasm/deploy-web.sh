#!/usr/bin/env bash
# Deploy the VXN2 web bundle to the vulpus-labs-site Hugo repo.
#
# Mirrors vxn-1/crates/vxn-wasm/deploy-web.sh: builds target/web-dist/ (cargo
# xtask web), copies it into the site's static tree at the synth subpath,
# ensures the root _headers carries this subpath's cross-origin isolation block
# (COOP/COEP — required for SharedArrayBuffer), then stages, commits, and
# pushes. The site auto-deploys from `main` via Netlify, so the push IS the
# deploy.
#
#   ./deploy-web.sh                      # build + copy + commit + push
#   SITE=~/elsewhere ./deploy-web.sh     # point at a different site checkout
#   NO_PUSH=1 ./deploy-web.sh            # build + copy + commit, but don't push
#   NO_BUILD=1 ./deploy-web.sh           # reuse the existing target/web-dist
#
# Unlike the vxn-1 script this *appends* its header block if missing rather than
# rewriting static/_headers wholesale — the file carries one block per synth and
# overwriting it drops the other synth's isolation headers.
set -euo pipefail

# --- config (override via env) ---
SITE="${SITE:-$HOME/src/vulpus-labs-site}"
SUBPATH="${SUBPATH:-products/vxn-2/web}"   # → https://vulpuslabs.com/products/vxn-2/web/
BRANCH="${BRANCH:-main}"

# Workspace root = four levels up (vxn-2/crates/vxn2-wasm/ → monorepo root),
# where target/ and the cargo workspace live.
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../../.." && pwd)"
DIST="$ROOT/target/web-dist"

[ -d "$SITE/.git" ] || { echo "error: $SITE is not a git checkout" >&2; exit 1; }

# 1. Build the bundle (unless reusing an existing one).
if [ -z "${NO_BUILD:-}" ]; then
  echo "==> building web bundle"
  ( cd "$ROOT/vxn-2" && cargo xtask web )
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

# Cross-origin isolation for the VXN2 web synth.
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
WASM_VER="$(cd "$ROOT" && git rev-parse --short HEAD)"
git commit -m "deploy: VXN2 web synth → /$SUBPATH/ (vxn-2 @ $WASM_VER)" \
  -m "Built from cargo xtask web; _headers sets COOP/COEP for SharedArrayBuffer."

if [ -n "${NO_PUSH:-}" ]; then
  echo "==> committed (NO_PUSH set — not pushing). Push manually to deploy."
  exit 0
fi
echo "==> pushing to $BRANCH (Netlify will deploy)"
git push origin "$BRANCH"
echo "==> done. Verify once live:"
echo "    curl -sI https://vulpuslabs.com/$SUBPATH/ | grep -i cross-origin"
