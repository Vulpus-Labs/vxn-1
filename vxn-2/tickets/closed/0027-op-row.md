---
id: "0027"
title: "Op row: algorithm picker overlay + op tabs + op detail"
priority: high
created: 2026-06-06
closed: 2026-06-07
epic: E003
---

## Summary

Wire the op-row's three interacting widgets:

1. Algorithm picker (32-cell overlay) → writes `algo` per layer.
2. Op tabs (op1..op6 strip) → switches the op-detail panel via
   the `set_op_tab` custom UI event. View-state only — not CLAP.
3. Op detail (21 params for the currently-selected op) → writes
   per-op CLAP params (`op{N}_ratio`, `op{N}_level`, etc.) where
   `{N}` is resolved from the active op tab.

The op tab's "carrier vs modulator" badge re-colours on algorithm
change. The EG and KS graphs in the op-detail panel are clickable
collapses (matches ADR 0001 §12 sketch) but they render the four
EG segments / five KS params using the 0026 graph primitive.

## Acceptance criteria

- [ ] Algorithm picker overlay:
      - 32 cells in a 4×8 grid, each rendering its DX7-canonical
        graph via `algo-diagram.js`.
      - Click selects the algorithm: dispatches
        `set_param { id: <algo_clap_id>, plain: N }`. CLAP id
        comes from the param table — `Upper algo` or `Lower algo`
        depending on `edit_layer`.
      - The current algorithm is highlighted. `applyViewEvents`
        for the algo id re-paints the highlight.
      - Toggle button on the op-row opens / closes the overlay.
        Escape key closes it.
- [ ] Op tabs (op1..op6):
      - Six buttons; clicking dispatches
        `set_op_tab { op: N }` custom event.
      - `Vxn2UiCustom::SetOpTab` lands on the controller; the
        custom handler updates the active-op cursor on the matrix
        view state (Vxn2Params) and emits
        `Vxn2ViewCustom::OpTabChanged { op }` for the page's
        own state machine (so the op-detail re-renders from the
        new op's slot).
      - Carrier/modulator badge re-colours from the current algo:
        a lookup table (the same 32-row DX7 routing table the
        engine already ships in `vxn2-engine::algo`) tells the
        page which ops carry, which modulate. Cheap to copy: 32
        × 6 booleans = 192 bytes in a JS literal.
- [ ] Op detail panel:
      - 21 params for the selected op surface as: 2 ratio /
        fixed-hz row, fine / detune / level / vel_sens /
        amp_sens row, EG graph (collapses R1..R4 + L1..L4), KS
        graph (collapses 5 KS params), pan, feedback (matches
        the mockup).
      - Each control writes to the CLAP id formed by
        `op_param_clap_id(layer, op, param_name)` — helper added
        to `vxn2-engine` if not already present.
- [ ] Cross-layer behaviour: when the user clicks "Lower" on the
      edit-layer toggle (on the op-row badge), the op tabs +
      op-detail panel re-render to source from the Lower-layer
      ops. The toggle dispatches `set_edit_layer { layer:
      "lower" }` custom event.
- [ ] Manual smoke: in a host, switching algorithm changes the
      timbre immediately; tabbing through op1..op6 surfaces the
      right param values; the EG / KS graph drag edits each op's
      envelope per the mockup ADR §12 sketch.

## Notes

- Edit-layer state is view-only — never sent to CLAP. The custom
  event sits in `Vxn2Params::set_edit_layer`; the page mirrors it
  locally (also reads `voicing_mode` to grey out Lower when
  voicing is Whole, but Lower params remain editable).
- Algorithm routing table: take from `vxn2-engine::algo` (defined
  in 0002). The engine source is the truth; if the JS literal
  ever drifts, it's a bug — add a test that dumps the engine
  table at build time and asserts the JS embed matches.
- Op-detail layout is dense (21 controls). The mockup compresses
  EG into one widget and KS into another via the graph primitive
  from 0026 — don't expand them back to 8 sliders. Editing
  density is the point.
- Feedback param goes to every op (VXN2 extension; DX7 had one
  feedback op per algo). Treat as a normal CLAP int 0..7 — the
  algo picker doesn't gate it.
