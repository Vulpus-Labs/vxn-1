---
id: "0027"
title: Preset browser UI
priority: medium
created: 2026-05-28
epic: E007
---

## Summary

Add a preset browser to the Vizia editor: list Factory (grouped by category) and
User presets, step prev/next, show the current preset name, load on selection,
and Save-As to the user directory. Sits on top of the format (0024), the factory
bank (0025) and the load/save path (0026). Decisions:
[ADR 0005](../../adrs/0005-vxn1-presets.md) §5–§6.

## Acceptance criteria

- [ ] A preset bar / panel in the editor showing the **current preset name** and
  **prev / next** steppers that walk the combined Factory+User list.
- [ ] A browser list/menu: **Factory** grouped by `meta.category` (read-only) and
  **User** (writable), each entry showing `meta.name`.
- [ ] Selecting a **Patch** loads it via 0026 into a target layer — Upper / Lower
  / "current edit layer" (the Upper/Lower edit toggle from ADR 0003 §6). Surface
  the target choice (default: current edit layer).
- [ ] Selecting a **Performance** loads the full instrument (both layers + global
  + key mode + split).
- [ ] **Save-As**: name field → writes a `.toml` to the user dir (0026). Saving
  the full instrument writes a `performance`; saving the edited layer writes a
  `patch` (offer both, or infer from key mode — decide and document).
- [ ] After a load, the editor's controls reflect the new values (they already
  re-read `SharedParams` via the `PollAutomation` idle path — confirm a bulk load
  repaints correctly).
- [ ] Load warnings from 0026 (unknown key / bad enum) shown non-fatally (status
  text or a transient line), not swallowed.
- [ ] No RT work on the audio thread; preset IO stays on the UI/main thread.

## Notes

- Build against the editor's existing idiom (CSS-styled panels, `SyncSignal` /
  `UiModel`, `on_idle` `PollAutomation`). A bulk preset load is many shared-param
  writes; the existing automation-poll repaint should already cover it — verify,
  since continuous relayout has bitten input before
  ([[vxn1-vizia-automation-relayout-input-stomp]]); a one-shot load is not
  continuous, so it should be fine, but check the controls stay interactive.
- Vizia drops clicks on tiny cursor drift ([[vxn1-vizia-no-click-slop]]) — use
  `on_press_down` for the stepper buttons / list entries.
- Keep it modest: category grouping + prev/next + Save-As. Search, tags,
  favourites and morphing are explicitly out (E007 scope).
- This is UI — verify by actually loading presets in a host and listening, not
  just by tests. Ask before any screen capture / opening GUI windows
  ([[ask-before-screen-capture]]).
