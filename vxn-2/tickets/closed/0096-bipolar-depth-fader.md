---
id: "0096"
title: "Bipolar depth fader: center fill, readout, double-click entry"
priority: medium
created: 2026-06-12
epic: E008
depends: []
---

## Summary

Seventh ticket of [E008](../../epics/open/E008-mod-matrix-completeness.md). The
matrix depth control is a bare `<input type="range">`
([mod-matrix.js:143-150](../../crates/vxn2-ui-web/assets/panels/mod-matrix.js#L143)) —
unlike every other slider it has no center bar, no value readout on
hover/drag, and no double-click numeric entry. Depth is **bipolar** (`[-1, 1]`,
0 = no modulation), so it should fill from the center and behave like the rest
of the faders ([fader.js](../../crates/vxn2-ui-web/assets/panels/fader.js)).

## Design

Replace the native range input with a fader built on the existing primitive,
adapted for a bipolar `[-1, 1]` value with center origin:

- **Center fill.** The standard fader fills bottom-up
  ([fader.js:128-131](../../crates/vxn2-ui-web/assets/panels/fader.js#L128)); a
  bipolar depth fader fills from the center toward the thumb — positive depth
  fills up from 0, negative fills down from 0. Add a `bipolar` mode to the fader
  primitive (or a thin `mm-depth` variant) that renders a center tick and a
  signed fill. Prefer extending `fader.js` so the value-pop / gesture / RAF
  throttle / drag-gate logic is shared, not reimplemented.
- **Value readout.** Show the depth amount in the shared `.value-pop`
  ([fader.js:86-103](../../crates/vxn2-ui-web/assets/panels/fader.js#L86)) on
  pointer-enter and during drag — formatted as a signed amount (e.g. `+0.42` /
  `-1.00`), ideally with the dest's unit once 0094 lands (so the readout reads
  in the dest's native full-scale, not raw normalized depth). Until 0094, show
  signed normalized depth.
- **Double-click entry.** Double-click opens the numeric text-input popup, same
  as other faders ([fader.js:224-227](../../crates/vxn2-ui-web/assets/panels/fader.js#L224)
  → `ctx.requestTextInput`). Typing a value sets depth and dispatches through the
  existing path. (User-confirmed: double-click is **text entry**, matching every
  other slider — not reset-to-zero.)
- **Shift-drag fine** (1/10 sensitivity) comes for free from the primitive.

**Dispatch wiring (unchanged routing).** The fader's `setNorm` / gesture
callbacks must drive the existing depth dispatch
([dispatchRow](../../crates/vxn2-ui-web/assets/panels/mod-matrix.js#L95) with
`{ depth }`), preserving the slot-1-8 CLAP-id path vs slot-9-16
`set_matrix_row` opcode split ([mod-matrix.js:124-135](../../crates/vxn2-ui-web/assets/panels/mod-matrix.js#L124)).
Map fader norm `[0, 1]` ↔ depth `[-1, 1]` (`depth = 2·norm − 1`). The
optimistic-update + drag-gate (`el.dataset.dragging`) must keep the snapshot
echo from stomping an in-progress drag ([paintRow](../../crates/vxn2-ui-web/assets/panels/mod-matrix.js#L257)
already guards on `document.activeElement`; the fader uses `dataset.dragging` —
reconcile so the matrix repaint respects it).

**CSS.** Center tick + signed fill styling under the `vxn-mm-*` namespace,
reusing fader track/thumb classes where possible.

## Acceptance criteria

- [x] Renders as a bipolar fader: `.vxn-mm-depth-center` tick at 0, signed
  `.fader-track-fill` grown from center, thumb tracks (`createBipolar.paint`).
- [x] Hover/drag readout in the shared `.value-pop` (signed `+0.42` / `-1.00`
  via `ctx.format`); walks live during drag with no engine echo (the fader owns
  `current` and repaints each `postNorm`). Unit-aware readout deferred — signed
  normalized depth ships now; enrich with the dest's native unit (0094 table)
  in a follow-up since DEST_GAIN isn't yet in the JS descriptor.
- [x] Double-click → `ctx.requestText` → `vxn.dispatchTextInput` → parse →
  `dispatchRow({depth})` (numeric entry, not reset-to-zero).
- [x] Shift-drag 1/10 (`sens = ev.shiftKey ? 0.1 : 1.0`).
- [x] Depth still routes through the existing CLAP (slot 1-8) / opcode (9-16)
  split — `dispatchRow` is unchanged; the fader only feeds it. No JS DOM harness
  exists, so the wiring (createBipolar used, range input gone, dispatch path,
  center-tick CSS) is asserted by `mod_matrix_depth_is_bipolar_fader`; the
  norm↔depth mapping (`depth = 2·norm − 1`) lives in `createBipolar`.
- [x] Snapshot repaint can't stomp an in-progress drag: `set()` no-ops while
  dragging and the fader sets `dataset.dragging`; `paintRow` calls
  `depthFader.set()` instead of writing a value directly.

## Notes

Pure UI ticket, no engine dependency — can land any time. The unit-aware readout
is nicer *after* 0094 (so the popup reads the dest's native full-scale), but the
fader itself doesn't depend on it; ship the signed-normalized readout and enrich
it when 0094 lands. Extending `fader.js` with a `bipolar` mode (vs forking) keeps
the gesture/throttle/text-entry behaviour identical to every other slider, which
is the whole point of the ask.
