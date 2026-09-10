---
id: "0389"
product: vxn-4
title: "vxn-4 preset bar and browser on the faceplate"
priority: medium
created: 2026-09-10
epic: E052
depends: ["0385", "0388"]
---

## Summary

Make the preset bar in the mockup real: walk the combined factory and user list,
browse it, save and save-as. Ported from vxn-1b's preset bar and two-pane
browser, over the `PresetStore` from [0385](0385-vxn4-user-preset-io.md) and the
shared controller's corpus machinery.

Last ticket of [E052](../../epics/open/E052-vxn4-editable-patches-and-faceplate.md).
With it, a vxn-4 patch is something a player can change, name, keep and get back.

## Acceptance criteria

- [ ] Prev / next walk the combined factory + user list, controller-side.
- [ ] Browse toggles a two-pane folders / presets panel with case-insensitive
      substring search across the selected folder.
- [ ] Save overwrites the current user preset, disabled on a factory patch;
      Save As opens a naming popup committing to the browser's selected folder.
- [ ] The bar shows the current preset name and a dirty indicator — a patch
      edited since load reads as modified.
- [ ] Loading a preset crosses to the audio thread as **one snapshot** on the
      topology ring, never as a stream of field edits.
- [ ] Loading a preset relabels and rescales the eight macro knobs from the
      patch, which is the behaviour the mockup demonstrates and the reason macro
      labels are patch data.
- [ ] A preset saved from the faceplate reloads identically: same audio, same
      knob positions, same macro labels.
- [ ] Warnings from the codec (unknown keys, unknown enum labels) surface in the
      status pill rather than being swallowed.

## Notes

The dirty indicator needs a definition, and the obvious one is wrong. "Any param
differs from the loaded preset" marks a patch dirty the moment a macro knob
moves, which is performance, not editing — and macro positions are host-automated
state that lives in the project, not in the preset. Probably: dirty means the
*patch* differs, with macro knob positions excluded. Settle it here.

Factory patches are read-only, so Save on one has to become Save As. Do not
silently redirect — disable Save and let Save As be the affordance, as vxn-1b
does.

The mockup's preset bar carries Browse / Save / Save As and a status pill
already, and the browser panel markup can be lifted from
[vxn-1b's faceplate](../../vxn-1b/crates/vxn1b-ui-web/assets/faceplate.html#L43-L59)
close to verbatim — same two-pane shape, same search box, same corpus events.
Nothing here should need new controller vocabulary.
