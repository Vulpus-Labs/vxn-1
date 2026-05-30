---
id: "0053"
title: HTML keys panel — mode selector, Upper/Lower toggle, split-point slider
priority: high
created: 2026-05-30
epic: E011
---

## Summary

Port the Keys panel from Vizia to HTML, finally taking the Vizia
overlay off the screen. Whole / Dual / Split mode selector, Upper /
Lower edit toggle (hidden in Whole, shown otherwise), split-point
slider over MIDI notes C0..C7 (shown in Split only).

## Acceptance criteria

- [ ] Keys panel renders in the Row 4 leftmost slot reserved by
      0040.
- [ ] Mode selector posts `UiEvent::SetKeyMode { mode }`; key mode
      sync via `ViewEvent::KeyModeChanged`.
- [ ] Upper / Lower toggle posts `UiEvent::SetEditLayer { layer }`;
      sync via `ViewEvent::EditLayerChanged` (added in 0045).
- [ ] Split slider visible only in Split mode; range C0..C7; posts
      `UiEvent::SetSplitPoint { note }`; readout shows note name
      ("C4", "F#5") matching Vizia's `note_name`.
- [ ] Reset-to-defaults button (the Vizia panel's "Reset" affordance)
      posts `UiEvent::ResetLayer { layer }` — controller calls
      `SharedParams::reset_patch_to_defaults`.

## Notes

Existing reset-button placement: under the Upper/Lower toggle in
Vizia. Keep the same shape.

After this ticket lands, the Vizia editor has no responsibilities
in the live UI; 0054 deletes the crate.
