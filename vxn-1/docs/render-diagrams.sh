#!/usr/bin/env bash
# Render every .mmd file under src/diagrams/ to a sibling .svg.
# Run before `mdbook build`. Requires @mermaid-js/mermaid-cli on PATH.
#
# Set MMDC_PUPPETEER_CONFIG to a puppeteer config JSON path to pass to
# mmdc via -p (needed in CI where chromium needs --no-sandbox).
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DIAGRAMS="$HERE/src/diagrams"

if [[ ! -d "$DIAGRAMS" ]]; then
  echo "no diagrams directory at $DIAGRAMS — nothing to render"
  exit 0
fi

EXTRA=()
if [[ -n "${MMDC_PUPPETEER_CONFIG:-}" ]]; then
  EXTRA+=(-p "$MMDC_PUPPETEER_CONFIG")
fi

shopt -s nullglob
for src in "$DIAGRAMS"/*.mmd; do
  out="${src%.mmd}.svg"
  echo "render $(basename "$src") → $(basename "$out")"
  mmdc -i "$src" -o "$out" --backgroundColor transparent "${EXTRA[@]}"
done
