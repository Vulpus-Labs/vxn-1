---
id: "0025"
title: Faceplate HTML/CSS port from mockup
priority: high
created: 2026-06-06
epic: E003
---

## Summary

Port `ui-mockup/index.html` (1515 lines, all-in-one) into
`vxn2-ui-web/assets/`: split CSS into `style.css`, keep one
`index.html` shell with element IDs / data attributes the panel JS
(0026) will bind to, and strip the inline placeholder values so the
page is data-driven by the params model hydrated over IPC.

After this ticket the faceplate looks right under the editor but
nothing moves yet. Knobs / faders show their default position; no
gestures fire. 0026 introduces the JS bindings; 0027 / 0028 layer the
overlays.

## Acceptance criteria

- [ ] `assets/style.css` carries every rule currently inline in
      `ui-mockup/index.html`. No visual regression vs the mockup
      (open both in a browser, eyeball at 1× and 0.75× zoom).
- [ ] `assets/index.html` is a thin shell: html / head (links
      `style.css`, inlines the bootstrap JS that defines
      `window.__vxn = { ... }`), body with the static page structure
      (banner, preset bar, op-row, gmod-row, perf-row), all
      placeholders removed.
- [ ] Each control container has:
      - `data-vxn-param="<machine id>"` for CLAP params (matches the
        engine's kebab-case ids from PARAMETERS.md).
      - `data-vxn-custom="<custom event key>"` for non-CLAP edits
        (matrix rows, op-tab switch, edit-layer toggle).
      - `data-vxn-section="<section name>"` so 0026 can pick its
        renderer (e.g. `op-detail`, `lfo1`, `delay`, `mod-matrix`).
- [ ] Faceplate viewport: 1024 × 772 logical px at the root `<div
      class="vxn-faceplate">`. Internal grid matches the mockup's
      sections row-for-row.
- [ ] All currently-inline event handlers (`onclick`, `oninput`, …)
      removed — JS in 0026 binds via querySelector against the data
      attributes.
- [ ] Inline SVG for the algorithm diagram + EG / KS graph stays in
      the markup as a `<svg>` template that 0027 will rewrite per
      patch.
- [ ] Asset path: every `<img>` / `<link>` is relative to the bundle
      root so `include_str!` / `include_bytes!` in `vxn2-ui-web`
      picks them up without runtime fs access.
- [ ] Sanity render: open `assets/index.html` in a browser, screen
      compare against `ui-mockup/index.html`; differences allowed
      only where placeholder values were stripped.

## Notes

- The ui-mockup HTML uses `style="..."` inline attributes for some
  per-row tweaks (panel offsets, badge colours). Move these to CSS
  classes; inline styles fight the param-driven re-renders 0026 will
  emit.
- Op tabs (op1..op6): make each a `<button data-vxn-op="N"
  data-vxn-custom="set_op_tab">` — the JS in 0027 will dispatch the
  `set_op_tab` custom event and re-render the op-detail panel from
  whichever op is active.
- Mod matrix overlay: keep the HTML scaffold in the main `index.html`
  hidden behind a `[hidden]` attribute. 0028 will toggle visibility
  via the `mod-matrix` button on the gmod-row.
- Algorithm picker overlay: same pattern — a hidden grid of 32
  algorithm cells in the markup that 0027 will populate with SVG
  graph thumbnails.
- Do NOT introduce a templating engine. Faceplate is small enough
  that data-attribute querySelector + textContent / style writes by
  the JS renderers will stay readable.
- This ticket carries no JS behaviour — every control sits inert
  until 0026 attaches behaviours. That's expected: the gate is
  visual parity with the mockup, not interactivity.
