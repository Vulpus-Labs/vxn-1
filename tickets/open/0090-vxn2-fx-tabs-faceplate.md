---
id: "0090"
product: vxn-2
title: "Tabbed FX panel on the faceplate (Phaser / Delay / Reverb)"
priority: medium
created: 2026-06-22
epic: E025
depends: ["0089"]
---

## Summary

Fourth ticket of [E025](../../epics/open/E025-vxn2-fx-tabs-phaser.md).
Replace the standalone delay + reverb panels with one tabbed FX panel
holding three tabs — Phaser / Delay / Reverb — and add the phaser
pane. Port vxn-1's `wireFxTabs` idiom and its test.

## Design

Files: `vxn-2/crates/vxn2-ui-web/assets/` — `index.html`, `main.js`,
`style.css`.

- HTML (`index.html:350-412`): replace `.delay-panel` + `.reverb-panel`
  with one `.fx-panel` containing a tab strip and three panes
  (`.fx-pane-phaser`, `.fx-pane-delay`, `.fx-pane-reverb`). Tab order
  = signal order: Phaser, Delay, Reverb. Move the existing delay /
  reverb controls into their panes verbatim; build the phaser pane
  with Rate / Depth / FB / Mix faders bound to the `phaser-*` ids.
- Tab idiom: port `wireFxTabs` from vxn-1's
  `vxn-1/crates/vxn-ui-web/assets/panels.js` into vxn-2's `main.js`.
  Pattern: tab buttons carry `data-tab="phaser|delay|reverb"`, panel
  carries `data-active-tab`, click swaps `.active` + repaints the
  newly-visible faders. Per-tab inline on/off switch follows the
  active tab's enable param (`phaser-on` / `delay-on` / `reverb-on`).
- CSS (`style.css`): tab-strip styling + `[data-active-tab="X"]`
  visibility gating, matching vxn-2's existing op-tab look
  (`index.html:65-67` already has an op-tab idiom for visual
  reference).
- Test: port `fx-tabs.test.js` from vxn-1's
  `vxn-ui-web/assets/__tests__/` into vxn-2's web test dir, adjusted
  for three tabs. Guards the click-swap contract + active-on-switch.

Hiding a tab does **not** bypass its DSP — an inactive FX still runs
if its `on` param is `1` (per epic out-of-scope).

## Acceptance criteria

- [ ] `.delay-panel` / `.reverb-panel` removed; one `.fx-panel` with
      three tabs renders.
- [ ] Phaser pane drives `phaser-rate/depth/feedback/mix` + `phaser-on`.
- [ ] Delay and reverb controls unchanged in behaviour, now in panes.
- [ ] `wireFxTabs` ported; clicking a tab swaps the visible pane and
      repaints its faders.
- [ ] Ported `fx-tabs.test.js` passes (web test runner).
- [ ] `cargo build -p vxn2-clap --release` loads with the tabbed FX
      panel visible and all three tabs reachable.

## Notes

vxn-2 has no existing FX-tab idiom (op-detail tabs exist but are
unrelated) — this is a straight port of vxn-1's proven pattern, not a
new design. Manual DAW check (Reaper) per [[verify-audio-in-reaper]]
after build.
