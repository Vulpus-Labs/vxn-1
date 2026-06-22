---
id: "0005"
product: vxn-1
title: HTML preset browser panel — folders / presets two-pane + search
priority: high
created: 2026-05-30
epic: E011
---

## Summary

Open the 0004 Browse button into a floating preset browser panel:
left pane lists folders (Factory categories + User folders), right
pane lists presets in the selected folder, search box filters by
name (case-insensitive substring match). Folder selection +
search are pure view state; everything else is controller-mediated.

This is a **redesign**, not a port. The Vizia browser's idioms
(`browser-pane`, `browser-section`, etc.) inform but don't bind.

## Acceptance criteria

- [ ] Panel opens / closes from 0004's Browse button.
- [ ] Left pane: Factory header + sorted categories, User header +
      "Uncategorised" first then sorted folders. Click selects.
- [ ] Right pane: name-sorted list of presets in the selected
      folder. Click loads (posts `UiEvent::LoadPreset { source }`).
- [ ] Search box: substring match on `meta.name`, lowercased,
      across the selected folder. Clear button resets.
- [ ] Panel scrolls if content overflows; ESC closes; clicking
      outside closes.
- [ ] Folder + preset corpus sourced from controller (`ViewEvent::
      PresetCorpusChanged` rebuilds the rendered lists).
- [ ] Currently-loaded preset highlighted in the right pane when
      its folder is selected.

## Notes

This ticket is "browse only". Rename / delete / move land in 0006;
drag-drop in 0007. The "redesign latitude" mostly cashes in there.
Browse-only is more constrained — the data shape is fixed by ADR 0006.

CSS-wise, the floating panel sits inside the WebView document (not
a separate native window). It can absolutely-position over the rows
without z-index drama; standard HTML.

## Close-out (2026-06-22)

- Two-pane browser shipped in `vxn-core-ui-web` `preset-browser.js`
  (`createPresetBrowser`, wired from `browser.js`): left folder pane
  (Factory categories + User folders), right name-sorted preset list,
  substring search box, ESC + click-outside close, scroll on overflow.
- Corpus sourced from controller (`ViewEvent::PresetCorpusChanged`
  rebuilds lists); preset click posts `UiEvent::LoadPreset`; loaded
  preset highlighted in its folder.
