---
id: "0209"
product: vxn-1b
title: "Compact 3-row faceplate HTML/CSS — fork VXN1, drop fixed mod panels"
priority: high
created: 2026-07-29
epic: E038
depends: []
---

> **SUPERSEDED (2026-07-31) by [[E039]] / [[0219]].** VXN1b gains dual-layer
> ([ADR 0002](../../vxn-1b/adrs/0002-vxn1b-dual-layer.md)); the single-patch
> **3-row** faceplate here is replaced by the **3-tab** layout (Layer 1 / Layer 2
> / FX-Mixer-Global). The forked HTML widgets/panels are **inherited by 0219** —
> only the 3-row layout + single-patch bindings are dropped. Closed as superseded.

## Summary

Build VXN1b's **compact 3-row faceplate**. Fork VXN1's faceplate assets
([vxn-ui-web/assets/](../../vxn-1/crates/vxn-ui-web/assets/)) into the currently
stub `vxn1b-ui-web` crate ([vxn1b-ui-web/src/lib.rs](../../vxn-1b/crates/vxn1b-ui-web/src/lib.rs)),
re-lay-out to 3 rows, and **delete** VXN1's fixed mod panels. This is the UI
payoff of [ADR 0001](../../vxn-1b/adrs/0001-vxn1b-overall-design.md) §7 —
[[E038]]'s first ticket.

VXN1b's ui-web crate is a stub today (lib.rs re-exports core types, sets
`EDITOR_WIDTH=760` / `EDITOR_HEIGHT=480` for the compact layout); no HTML/CSS/JS
yet. VXN1's faceplate is 4 rows with `faceplate.html` + `faceplate.css` +
`panels/*.js` + `bridge.js`/`browser.js`/`dispatch.js`.

## Design (ADR 0001 §7)

**3-row layout:**

- **Row 1** — Osc1 · Osc2 · Mixer · Filter
- **Row 2** — LFO1 · LFO2 · Env1 · Env2
- **Row 3** — Voice · FX · Master

**Remove** VXN1's Pitch Mod / PWM Mod / Cross Mod / Mod Wheel / Pitch Wheel /
Filter Mod panels — all fixed routing now lives in the matrix overlay ([[0210]]).
Keep the source-shaping panels (LFO/Env). Keep the preset bar
(Prev/Next/Browse/Save/Save As + status pill); it gains the `Mod Matrix · N`
trigger in [[0210]] and the FX tabs are [[0211]] (this ticket ships a plain FX
panel placeholder or the existing tab shell, whichever is cleaner to wire).

**Bindings.** Wire each control's `data-param` to the vxn1b param names
([vxn1b-engine/src/params.rs](../../vxn-1b/crates/vxn1b-engine/src/params.rs)):
osc/mixer/filter/LFO/env/voice/master groups. Matrix depths + FX params bind in
[[0210]]/[[0211]].

**MVC.** Reuse the VXN1 dirty-bitset dispatch idiom (view never reads model). No
new discipline here — just fewer panels.

## Acceptance

- `vxn1b-ui-web` ships `faceplate.html`/`.css` + panel JS forked from VXN1,
  re-laid to 3 rows.
- The six fixed mod panels are gone; source-shaping LFO/Env panels remain.
- Every non-matrix, non-FX control binds to a real vxn1b param name.
- Contract/token tests (control→param map, CSS token presence) pass, mirroring
  VXN1's `__tests__` fixtures.
- `vxn1b-clap` implements the CLAP GUI extension — `open_editor` +
  view-event flush timer, mirroring VXN1's `gui.rs` — so the faceplate opens in
  a DAW (the E036 shell had no GUI; folded in here).
- Loads without JS errors; faceplate reads visibly simpler than VXN1.
</content>
