#!/usr/bin/env bash
# Render every .mmd file under src/diagrams/ to a sibling .svg.
# Run before `mdbook build`. Requires @mermaid-js/mermaid-cli on PATH.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DIAGRAMS="$HERE/src/diagrams"

if [[ ! -d "$DIAGRAMS" ]]; then
  echo "no diagrams directory at $DIAGRAMS — nothing to render"
  exit 0
fi

shopt -s nullglob
for src in "$DIAGRAMS"/*.mmd; do
  out="${src%.mmd}.svg"
  echo "render $(basename "$src") → $(basename "$out")"
  mmdc -i "$src" -o "$out" --backgroundColor transparent
done
