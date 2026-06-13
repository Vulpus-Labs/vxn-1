---
id: "0095"
title: "Matrix-table coherence validation: red text on invalid routings"
priority: high
created: 2026-06-12
epic: E008
depends: ["0090"]
---

## Summary

Sixth ticket of [E008](../../epics/open/E008-mod-matrix-completeness.md). The
matrix overlay lets a user pick any source/dest combination; many are incoherent
(per the [coherence rule](../../epics/open/E008-mod-matrix-completeness.md#the-coherence-rule)).
Compute coherence per row from the tier metadata exported by
[0090](0090-matrix-granularity-metadata.md) and render the offending source/dest
text **red** with an explanatory tooltip — without blocking the edit (old patch
blobs must still load, and the user keeps final say).

## Design

In [mod-matrix.js](../../crates/vxn2-ui-web/assets/panels/mod-matrix.js):

- Consume the coherence lookup from `window.__vxn.matrix` (the table/predicate
  result exported by 0090) — **do not re-derive the rule in JS**, read the
  exported verdict so engine and UI never drift.
- After each `dispatchRow` / `paintRow`, evaluate `coherence(source, dest)` for
  the row's current pair and toggle a class on the row (and/or the two selects):
  - `Ok` → no class (normal).
  - `TierCollapse` / `SelfRate` / `Degenerate` → add `.vxn-mm-invalid`; set the
    select(s) `title` to the reason, e.g.:
    - TierCollapse: *"per-lane source can't drive a per-stack target — value
      collapses to lane 0"*
    - SelfRate: *"an LFO can't modulate its own rate"*
    - Degenerate: *"voice-idx reads 0 at the collapsed lane — no effect"*
- An empty slot (`source = none` or `dest = none`) is never flagged.

CSS (faceplate stylesheet): `.vxn-mm-invalid select { color: var(--vxn-error,
#e0564b); }` — red text on the source/dest selects. Keep depth/curve/active
untouched (the routing is what's invalid, not the amount). Match the existing
`vxn-mm-*` class naming and the faceplate's error/warn color token if one exists
(grep the stylesheet; add `--vxn-error` if not).

Re-validate on every relevant edit: source change, dest change, snapshot repaint
([paintRow](../../crates/vxn2-ui-web/assets/panels/mod-matrix.js#L246)), and
initial `renderAll`. A slot loaded from a preset that happens to be incoherent
(e.g. the `voice-rand → lfo2-phase` in the Mark II E-Piano factory patch, now
*coherent* after 0091, but other legacy combos may not be) shows red on load.

## Acceptance criteria

- [x] Incoherent rows (TierCollapse / SelfRate / Degenerate) render source/dest
  text red with a reason tooltip; coherent rows render normally. `.vxn-mm-invalid
  select { color: var(--vxn-error) }` + `validateRow` toggling the class. The
  representative-pair verdicts are pinned engine-side (0090 grid +
  `build_matrix_lists_json_carries_tiers_and_coherence`: `voice-rand→lfo2-rate`
  tier-collapse, `voice-rand→lfo2-phase` ok, `lfo2→lfo2-rate` self-rate,
  `voice-idx→cutoff` degenerate, `velocity→cutoff` ok); no JS DOM harness exists,
  so the panel wiring is asserted by `mod_matrix_panel_wires_coherence_validation`.
- [x] The verdict is read from the exported coherence table (0090), not
  recomputed in JS — `verdictFor` indexes `window.__vxn.matrix.coherence`; the
  degenerate `voice-idx` verdicts only the engine table knows are exported + tested.
- [x] Setting an incoherent routing is **not blocked** — `dispatchRow` is
  unchanged; `validateRow` only toggles the class/tooltip.
- [x] Validation runs on source/dest edit (in `dispatchRow`), snapshot repaint +
  initial render (in `paintRow`); the bin clear routes through `dispatchRow` with
  an all-`none` row → verdict `ok` → flag cleared.
- [x] Empty slots never flag (`verdictFor` guards `source`/`dest` id 0).
- [x] No change to the depth dispatch paths (slot 1-8 CLAP / 9-16 opcode).

## Notes

Red-text-not-block is deliberate: the matrix is a creative tool and some
"incoherent" routes are merely degenerate rather than dangerous (a collapsed
lane-0 read still does *something* for `voice-spread`/`voice-rand`). Flagging
teaches the user the granularity model without trapping them. A follow-up could
gray out incoherent dest options per selected source in the dropdown, but the
minimum bar for this ticket is the red text + tooltip the user asked for.
