---
id: "0098"
title: Faceplate — FX tab panel + Drift knob in Master
priority: medium
created: 2026-06-06
epic: E018
---

## Summary

Replace the four row-4 FX panels (Chorus / Delay / Reverb +
the planned Phaser) with one tabbed FX panel — vertical tab
selector on the left, single content area on the right showing
the active tab's controls. Tabs in order: **Phaser / Chorus /
Delay / Reverb**.

Add a **Drift** knob to the Master panel alongside Tune /
Volume.

Header on/off switch on the FX panel follows the active tab's
enable param (`phaser_on` / `chorus_on` / `delay_on` /
`reverb_on`) — switching tabs swaps which param the header
switch is bound to.

## Acceptance criteria

- [ ] `crates/vxn-ui-web/assets/faceplate.html`: rows 4 changes
      from four FX panels + Master to **one FX panel + Master**.
      The FX panel uses `data-control="tabs"` (new primitive)
      with four tab definitions.
- [ ] Tab strip is a vertical column on the left of the FX
      panel, ~24px wide, four labels (PHASER / CHORUS / DELAY /
      REVERB). Active tab visually distinct (border-left accent
      or background tint — match existing accent idiom).
- [ ] Tab body shows only the active tab's controls:
      - **Phaser**: Rate / Depth / FB / Mix faders
      - **Chorus**: Rate / Depth / Mix faders (unchanged)
      - **Delay**: Time / FB / Mix faders + Sync / P-Pong strip
        (unchanged)
      - **Reverb**: Size / Decay / Damp / Mix faders (replaces
        Type/Depth/Mix)
- [ ] Header on/off switch dynamically binds to the active
      tab's `*_on` param. Switching tabs unbinds the previous
      and rebinds the new param in one frame.
- [ ] Master panel gains a `data-control="fader"
      data-param="master_drift" data-label="Drift"` cell
      between Volume and the bottom strip (or right of Volume —
      match the visual density of the existing tune/volume
      pair).
- [ ] CSS: `.panel-tabs` (vertical strip) + `.panel-tab.active`
      added. Width budget for row 4 redistributes: previous four
      FX panels claimed ~4 panel widths; the tabbed FX panel
      takes ~2.5 widths, master can grow to ~1.5 to fit the new
      Drift knob.
- [ ] JS dispatch (`dispatch.js` or `panels.js`): tab click
      sets a `data-active-tab` attribute on the FX panel,
      visibility toggled via CSS `[data-active-tab="phaser"]
      .tab-phaser { display: flex; }` (etc.) — pick whichever
      pattern matches the codebase's existing show/hide idiom.
- [ ] Existing JS tests updated. Add one test for tab
      switching: click tab → only that tab's controls visible →
      header switch param ref updated.
- [ ] Smoke-load in the running plugin (per [[ask-before-screen-capture]],
      ask before running the GUI smoke check).

## Notes

The vertical tab strip is a new primitive. Keep it minimal —
text labels rotated 0° (not vertical text); the panel is short
enough that 4 horizontal labels stacked vertically fits. If
height is tight, abbreviate (PHS / CHR / DLY / RVB) — judge by
ear in the live preview.

Header-toggle binding swap is the novel JS bit. The cleanest
pattern is:

```html
<div class="panel" data-name="FX" data-header-toggle data-active-tab="phaser">
  <div class="panel-header">
    <div class="panel-header-toggle-slot" data-control="header-switch" data-tab-bound-param></div>
    <div class="panel-header-title">FX</div>
  </div>
  <div class="panel-tabs">
    <button class="panel-tab" data-tab="phaser" data-on-param="phaser_on">PHASER</button>
    <button class="panel-tab" data-tab="chorus" data-on-param="chorus_on">CHORUS</button>
    …
  </div>
  <div class="panel-body">
    <div class="tab-body tab-phaser">…</div>
    <div class="tab-body tab-chorus">…</div>
    …
  </div>
</div>
```

…and the dispatch reads `data-on-param` off the active tab's
button into the header-switch slot. Bikeshed welcome.

Tab switching is purely visual — DSP for inactive tabs still
runs if its `*_on = 1`. Tabs are a UI scrolling device, not a
bypass.

Per [[vxn1-vizia-hoverable-propagates]] and the wider vizia
post-mortem, the HTML faceplate is the live UI; this ticket has
no vizia work.

## Touches

- `crates/vxn-ui-web/assets/faceplate.html`
- `crates/vxn-ui-web/assets/faceplate.css`
- `crates/vxn-ui-web/assets/panels.js` (or `dispatch.js` —
  whichever owns control mount)
- JS tests
