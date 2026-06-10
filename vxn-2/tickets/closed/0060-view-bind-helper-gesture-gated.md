---
id: "0060"
title: "View bind helper: extract bindGestureGated, retrofit existing primitives"
priority: medium
created: 2026-06-10
epic: E005
depends: ["0058"]
---

## Summary

Sixth and final ticket of [E005](../../epics/open/E005-dirty-bitset-pump.md).
Mid-drag suppression of incoming `param_changed` events lives in the
view — only the view knows which widget is being dragged. The pattern
exists ad hoc in `paintRow`
([mod-matrix.js:202-204](../../crates/vxn2-ui-web/assets/panels/mod-matrix.js#L202))
as `if (document.activeElement !== r.source)` etc. Lift it into a
shared helper so every primitive inherits it.

Per [ADR 0003](../../adrs/0003-dirty-bitset-diff-pump.md) §"What
survives" → "Mid-drag suppression": the pump is dumb; the view filters.
Today's gesture gate in the pump (kept for transition safety in 0057)
can move here, leaving the pump truly source-agnostic.

## Acceptance criteria

- [ ] `bindGestureGated(domNode, setFn)` helper exists in
  `vxn2-ui-web/assets/main.js` (or a shared `assets/util/` module if
  one is created). Wraps `setFn(plain, norm, display)` with a guard
  that drops the update when:
  - `document.activeElement === domNode` (the user is focused on
    the input — typing, range-input drag), OR
  - an explicit `domNode.dataset.dragging === "1"` flag is set
    (used by pointer-drag knob widgets that don't claim focus).
- [ ] Existing primitive binders (`bindFaders`, `bindWaveKnobs`,
  `bindButtonGroups`, `bindBoolToggles`, `bindPitchEg`) route their
  `set` callbacks through `bindGestureGated`. Where the existing
  callback already does some form of activeElement check, replace
  the ad-hoc check with the helper.
- [ ] Pointer-drag knobs (or any widget that uses `mousedown` /
  `mousemove` / `mouseup` instead of focus) set
  `dataset.dragging = "1"` on `mousedown` and remove it on `mouseup`.
  The helper sees it and drops events for the dragging widget.
- [ ] `mod-matrix.js paintRow` updated: the explicit
  `document.activeElement !== r.source` checks become unnecessary
  because the row's bound primitive `set` callbacks go through
  `bindGestureGated`. (Or keep the row-level check as belt-and-braces
  if the row binding doesn't go through the standard primitive
  pipeline.)
- [ ] Pump-side gesture gate (kept in 0057 for transition safety)
  is **removed** in this ticket. Document the migration: gate moved
  from `drain_dirty_bits` to the view's bind layer. The pump becomes
  truly source-agnostic.
- [ ] `SharedParams.gestures` bitset survives — still drives CLAP
  `gesture_begin` / `gesture_end` events out to the host. Unchanged.
- [ ] Manual test (Reaper):
  - Drag a fader. The host echoes the value back as `param_changed`
    on every tick. The fader doesn't fight the drag — the bind
    helper drops the incoming events while the input is focused.
  - Release the fader. The next tick's `param_changed` lands and
    the fader settles at the authoritative value.
  - Bind host automation to the same fader and start it moving.
    Without touching the fader: the fader follows the automation.
    Mid-drag: the user's drag wins (bind helper drops the automation
    echo) until release.
  - Pointer-drag a knob (not a focusable range input). Same
    expectations via `dataset.dragging`.
- [ ] `cargo test -p vxn2-ui-web` green. No regression in the
  serialise/parse JS-bridge tests.

## Notes

This ticket's win is consistency, not behaviour. Today's `paintRow`
check protects matrix-overlay widgets but not the rest of the
faceplate. Lifting it means every primitive gets the same protection
for free, and the pump simplifies.

The helper is small (~10 lines of JS). The retrofit is mechanical —
each primitive's `register(id, { set })` callback gets wrapped.
Sequence the changes per binder so one diff = one binder's
retrofit; or batch into one PR if the diff fits.

Edge case: range inputs receive `input` events on programmatic
`.value = "..."` writes (browser behaviour depends on engine, but
WebView2 / WKWebView typically do). The helper gates `set` callbacks,
not the `.value` write, so this isn't an issue — but verify on the
target WebView before declaring victory.

Once 0060 ships, the unidirectional MVC from ADR 0003 §"Architectural
framing" is fully realised: pump is dumb, view filters, controller
routes inputs, model is truth. Future fields inherit the discipline
by default.
