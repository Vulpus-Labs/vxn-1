---
id: "0148"
product: vxn-2
title: "Dynamics — add 'Dyn' tab to the FX panel"
priority: medium
created: 2026-06-24
epic: E028
depends: ["0147"]
---

## Summary

Fourth ticket of [E028](../../epics/open/E028-vxn2-fx-dynamics-block.md).
Add a fourth tab `Dyn` to the existing tabbed `.fx-panel` (E025 / 0090),
placed **left of Phaser** so the tab order matches the signal order
(Dyn / Phaser / Delay / Reverb). Per-tab on/off switch bound to
`dyn-on`; knobs for threshold / ratio / attack / release / makeup /
drive / mix.

## Design

Files: `vxn-2/crates/vxn2-ui-web/assets/`:

- `index.html` — `.fx-panel` (lives where the closed 0090 placed it).
  Add a fourth tab button (`<button class="fx-tab" data-tab="dyn">Dyn</button>`)
  as the **first** tab, and a new `.fx-pane-dyn` containing:
  - one `.fx-tab-switch` (`.bgrp-toggle`) bound to `dyn-on`;
  - faders/knobs bound to `dyn-threshold`, `dyn-ratio`, `dyn-attack`,
    `dyn-release`, `dyn-makeup`, `dyn-drive`, `dyn-mix`.
- `main.js` — no `wireFxTabs` changes needed (it iterates `.fx-tab`
  children, so a fourth tab is picked up automatically). The
  `repaintFxPane` callback already handles arbitrary pane names.
- `style.css` — tab-strip already supports four tabs (`flex` layout);
  spot-check the active-tab indicator at four-tab width and adjust if
  cramped.
- `lib.rs` — no bundle splicing change (fx-tabs.js is already
  included).

**Default active tab.** `.fx-panel` currently boots
`data-active-tab="phaser"` (per the closed 0090 ticket's HTML). Leave
that as the default — opening a saved patch shouldn't surface the
fourth tab unless the user clicks it.

**Test.** Extend `vxn-2/crates/vxn2-ui-web/assets/__tests__/fx-tabs.test.js`
(ported from vxn-1 in 0090) to assert four tabs, click-swap reaching
the `dyn` pane, and the per-tab switch toggling `dyn-on`.

## Acceptance criteria

- [ ] `.fx-panel` shows four tabs in signal order: Dyn / Phaser /
      Delay / Reverb.
- [ ] `.fx-pane-dyn` contains the on/off switch + seven param
      knobs/faders, all bound to the `dyn-*` ids.
- [ ] Clicking the Dyn tab activates the pane (CSS visibility) and
      `wireFxTabs` repaints its faders from cached norms.
- [ ] Default active tab unchanged (`data-active-tab="phaser"`).
- [ ] Extended `fx-tabs.test.js` passes (web test runner — 4-tab
      assertions).
- [ ] `cargo build -p vxn2-clap --release` loads with the four-tab
      FX panel, all dynamics controls driving DSP.

## Notes

The Dyn pane should follow the same fader/knob conventions as the
other panes for layout consistency — see the closed
[0090](../../tickets/closed/0090-vxn2-fx-tabs-faceplate.md) close-out
for the existing pane structure.

Manual Reaper check per [[verify-audio-in-reaper]] — confirm
host-automation lanes show the eight `dyn-*` ids and the fade-on /
fade-off on `dyn-on` is click-free.

Final ticket of E028 — close the epic via `/close-epic E028` once
this lands.
