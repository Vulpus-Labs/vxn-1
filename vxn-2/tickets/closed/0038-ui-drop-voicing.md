---
id: "0038"
title: UI — drop voicing-mode + edit-layer, flatten param binding
priority: medium
created: 2026-06-09
epic: E004
---

## Summary

Sixth ticket of [E004](../../epics/open/E004-single-layer-collapse.md).
Rip the voicing-mode picker, the Upper/Lower edit toggle, and the
`editLayer` state machine out of the HTML faceplate and panel JS. After
this ticket the faceplate has no per-layer UI; every panel binds
directly to flat parameter IDs.

Per [ADR 0002](../../adrs/0002-drop-dual-layer.md). Depends on **0035**
(flat IDs) and **0037** (flat events).

## Acceptance criteria

- [ ] `crates/vxn2-ui-web/assets/index.html`:
  - [ ] Voicing-mode button group (lines ~203-215) deleted.
  - [ ] Edit-layer toggle deleted.
  - [ ] Split-point fader / control deleted.
  - [ ] Layout adjusted so the row that previously housed
        voicing-mode no longer carries a gap; nearby controls reflow.
- [ ] `crates/vxn2-ui-web/assets/main.js`:
  - [ ] `editLayer` global / module state removed (line ~9).
  - [ ] `upper-` / `lower-` ID prefixing removed from
        `paramId()` / `bind()` helpers (lines ~145-151).
  - [ ] `voicing-mode` change listener removed (lines ~373-382).
  - [ ] `edit_layer_changed` event listener removed (lines ~408-412).
  - [ ] Per-block param reconciliation no longer references
        `voicing-mode` / `split-point` (lines ~505-506).
- [ ] `crates/vxn2-ui-web/assets/panels/mod-matrix.js`:
  - [ ] Layer-prefix concatenation (lines ~60-91, ~218-269) removed.
  - [ ] `onEditLayerChanged()` handler removed.
  - [ ] Matrix renders against the single-table snapshot from 0037.
- [ ] `crates/vxn2-ui-web/assets/panels/op-row.js`:
  - [ ] `currentLayer()` method removed.
  - [ ] Algo + op-tab dispatch no longer takes a layer arg.
- [ ] `crates/vxn2-ui-web/assets/bootstrap.js`:
  - [ ] `upper` / `lower` matrix snapshot initialisation (lines ~46-49)
        replaced with a single `matrix` array.
  - [ ] `editLayer: "upper"` initialisation removed.
- [ ] `crates/vxn2-ui-web/assets/style.css`:
  - [ ] `.edit-layer-toggle.muted` rule (lines ~229-231) deleted.
  - [ ] Any other `.edit-layer-*` or `.voicing-*` styles deleted.
- [ ] `crates/vxn2-ui-web/src/lib.rs`:
  - [ ] `use ...::Layer` removed (line ~18).
  - [ ] `set_edit_layer` JSON parsing (lines ~193-197) deleted.
  - [ ] `EditLayerChanged` / `MatrixSnapshot` JSON enc/dec (lines
        ~230-264) flattened: no `layer` field, single matrix array.
  - [ ] Tests at lines ~366-432 + ~543-568 updated or deleted to match.
- [ ] `ui-mockup/index.html`:
  - [ ] Voicing-mode button group (line ~791) deleted.
  - [ ] Mockup stays the layout source-of-truth per ADR 0001 §11 —
        update it in lockstep with the production faceplate.
- [ ] Editor smoke test (`crates/vxn2-clap/tests/editor_smoke.rs`)
      green.
- [ ] Manual: load the plugin in a CLAP host, open the editor, confirm:
  - no voicing-mode picker
  - no edit-layer toggle
  - all op-row + matrix interactions write through flat IDs
  - DAW parameter list shows 179 entries (no `Upper /` or `Lower /`
    module prefix)

## Notes

This is the most cross-cutting ticket in the rip. Touches HTML, CSS, JS
panels, and the Rust JSON bridge. Expect ~300-400 LoC removed across
the JS layer.

The `editLayer` state machine in `main.js` was the central cause of
panel-binding complexity — every panel had to consult it to know which
prefix to attach. Removing it simplifies every panel's param-binding
path. Watch out for stale `${layer}-` template literals in panels not
explicitly listed above; grep for `${.*layer}` and `upper-` /
`lower-` across `assets/` to catch them.

The split-point control was a fader (per ADR 0001 §8 + PARAMETERS.md).
Its space in the layout goes back to the surrounding panel — the
"Voicing" row in `ui-mockup/index.html` collapses entirely.

The voicing-mode UI used a button-group (Whole / Layer / Split). The
button-group widget itself stays (used elsewhere — LFO shape,
stack-distrib, etc.); only this instance disappears.

CSS palette tokens used only by `.edit-layer-toggle.muted` can be
dropped if no other rule uses them. Audit before deletion.

The `bootstrap.js` initialisation now seeds a flat `matrix` array of
16 slots, not an `{ upper, lower }` object. The matrix overlay
(`panels/mod-matrix.js`) reads from `state.matrix` directly.

If the editor smoke test breaks on a missing JSON field
(`MatrixSnapshot.layer`, `EditLayerChanged`), confirm 0037 has shipped
on the same branch — the Rust bridge must already speak the flat shape
before this ticket can land.
