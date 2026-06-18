---
id: "0070"
product: vxn-2
title: "Expose the stack-pitch dest in the faceplate mod matrix"
priority: medium
created: 2026-06-18
epic: E022
depends: ["0068"]
---

## Summary

Surface the new `OpN Stack Pitch` destinations in the faceplate mod-matrix
UI so a user can pick "Op N stack pitch" as a route target. View-only change
over the existing dest picker; the engine does the work (0069).

## Design

- The faceplate is the HTML/JS UI in
  [vxn2-ui-web](../../vxn-2/crates/vxn2-ui-web/assets/). The mod-matrix dest
  list is driven by `DEST_LABELS` from the engine (0068) — confirm the
  picker enumerates dests from the shared list rather than a hardcoded JS
  copy; if hardcoded, add the six entries and file a follow-up to data-drive
  it.
- **Grouping.** Keep `Op N Pitch` and `Op N Stack Pitch` adjacent in the
  dest dropdown so the relationship reads at a glance (per-op vs whole-stack
  pitch).
- **Fixed-target hint (optional, nice-to-have).** When the selected stack
  target op is currently in Fixed mode, the route is a no-op (0067/0069).
  If cheap, show an inline hint ("fixed op — stack pitch inert"); otherwise
  defer to a follow-up. Do not block the ticket on it.
- MVC discipline ([[vxn2-mvc-discipline]], ADR 0003): the view only renders
  the dest choice and emits the route-edit opcode; it must not compute the
  component or read model state to decide the hint — derive any hint from a
  dirty-bitset-pumped flag, not a direct model read.

## Acceptance criteria

- [ ] The mod-matrix dest picker lists `Op 1..6 Stack Pitch`, grouped beside
      the per-op pitch dests.
- [ ] Selecting a stack-pitch dest emits the same route-edit opcode shape as
      any other dest (source/dest/depth); round-trips through save/load.
- [ ] No hardcoded dest list drifts from the engine's `DEST_LABELS` (either
      data-driven, or a follow-up filed).
- [ ] Manual check: route LFO/pitch-EG → `Op N Stack Pitch`, hear the whole
      branch bend in tune.

## Notes

- Out of scope: a dedicated "stack" toggle widget on pitch routes — the dest
  enum carries the intent (0068), so the existing picker suffices.
- If the fixed-target hint lands here, keep it presentational only.

## Close-out (2026-06-18)

- Dest picker is data-driven from the engine's `DEST_LABELS`/`DEST_NAMES` via
  `build_matrix_lists_json` → `window.__vxn.matrix.dests`, so the six new dests
  appear automatically — no hardcoded JS list to drift.
- `destDisplayOrder()` in
  [mod-matrix.js](../../vxn-2/crates/vxn2-ui-web/assets/panels/mod-matrix.js)
  reorders the dropdown so each `opN-stack-pitch` renders directly after
  `opN-pitch`; option *values* stay the wire dest id, so the route-edit opcode
  shape (`set_matrix_row` source/dest/depth) and `paintRow` value-set are
  unchanged and save/load round-trips.
- Fixed-target inline hint: deferred (ticket marked it optional / nice-to-have).
- Manual DAW listening check (route LFO → Op N Stack Pitch, hear the branch
  bend in tune) — **pending**, not runnable headless.
