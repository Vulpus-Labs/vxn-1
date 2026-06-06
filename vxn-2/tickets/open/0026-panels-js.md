---
id: "0026"
title: "Panels JS: wave-knob / fader / button-group + section renderers"
priority: high
created: 2026-06-06
epic: E003
---

## Summary

Introduce the panel primitives JS (`assets/panels/{knob,fader,button-group,graph,algo-diagram}.js`)
and the per-section renderers that bind them to the params model
hydrated over IPC. After this ticket every static-section control
(LFO1, LFO2, Pitch EG, Mod Env, Voice / Stack, Delay, Reverb,
Master) moves the engine. Op-row complexity (0027), mod matrix
(0028), preset bar (0029) layer on top.

The page exposes one global `window.__vxn` with:
- `applyViewEvents(events)` — called from Rust to apply view
  updates. Dedupe-by-id already happened in the Rust bridge.
- `applyPresetCorpus(corpus)` — called for browser panel updates
  (no-op until 0029 wires the panel).
- `params` — the hydrated descriptor table (id → desc).
- `dispatch(opcode, payload)` — posts an IPC message to Rust.

## Acceptance criteria

- [ ] `assets/panels/knob.js`: wave-knob primitive. Mouse / touch
      drag → normalised gesture → emits `begin_gesture` →
      `set_param_norm` (debounced to ~animation-frame rate) →
      `end_gesture`. Double-click pops the native numeric-entry
      popup via `request_text_input` (handler wired in 0030). SVG /
      canvas — pick whichever renders cleanest at 1× and 0.75×.
- [ ] `assets/panels/fader.js`: linear / log fader primitive. Same
      gesture protocol as knob. Hosts the taper math by reading
      `desc.taper` from the hydrated params table.
- [ ] `assets/panels/button-group.js`: enum / bool select. Click
      cycles or selects; emits `set_param { plain: <variant
      index> }` for enum, `set_param { plain: 0|1 }` for bool. No
      gesture brackets (single discrete edit).
- [ ] `assets/panels/graph.js`: ADSR / Pitch-EG segment graph. Each
      handle drags one (rate, level) pair; emits batched
      `set_param_norm` calls bracketed by one
      `begin_gesture` / `end_gesture` across all four segments. EG
      visualisation reads back from `applyViewEvents` to redraw on
      host automation.
- [ ] `assets/panels/algo-diagram.js`: read-only SVG renderer for
      the current algorithm number. Used twice: small thumbnail in
      the op-row badge, full graph in the algorithm picker overlay
      (0027 binds the picker grid).
- [ ] `assets/main.js`: bootstrap.
      - Reads `data-vxn-param` / `data-vxn-section` attributes from
        the DOM.
      - Resolves each `data-vxn-param` to a `ParamId` via the
        hydrated `params` table (param machine id → CLAP id).
      - Instantiates the right primitive per `data-vxn-section`
        renderer (e.g. `lfo1` → wave-knob + fader + button-group).
      - Calls `dispatch("ready")` once the page is bound.
- [ ] `applyViewEvents([{kind: "param_changed", id, plain, norm,
      display}, ...])` writes through to each primitive's `set()`
      method — only the latest plain / norm / display lands per id
      per batch.
- [ ] Manual smoke: in a host, every fader / knob on LFO1, LFO2,
      Pitch EG, Mod Env, Voice (assign mode / legato / glide),
      Stack (density / detune / spread / phase / distrib), Delay,
      Reverb, Master changes the audio output within one tick.
- [ ] Host automation on any of these CLAP ids moves the faceplate
      control during playback (0024's `last_seen` diff pump path is
      live).

## Notes

- Resist the temptation to depend on a framework. VXN1 ships
  hand-rolled vanilla JS for the same reason: tiny diff against
  what wry's WebView can run cold-start, no bundler overhead, no
  audit pressure on every dependency.
- Knob / fader gesture protocol matches the shared opcode set in
  `vxn-core-ui-web::parse_ui_event_default`: `begin_gesture`,
  `set_param_norm`, `end_gesture`. No VXN2-specific opcodes here —
  every static control is a plain CLAP param.
- Debounce `set_param_norm` to 16 ms during a drag (one event per
  tick is plenty; the audio thread reads the latest value at the
  top of each block). The first / last gesture edits ALWAYS go
  through unthrottled to keep tap-and-hold gestures crisp.
- Op-row primitives (op tabs + op detail panel + algorithm picker)
  layer in 0027. This ticket lands the EG / KS graph primitive but
  doesn't bind it to op-detail panel slots — that's 0027's job.
- Mod matrix overlay primitives (per-row dropdowns + depth knob +
  curve picker + active toggle) layer in 0028.
- Preset bar primitives (display, prev/next, Save / SaveAs /
  Browse) layer in 0029.
