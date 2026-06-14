---
name: find-in-source
description: Locate code in the vxn-2 source tree using a map of crate/file responsibilities, so searches start in the right place instead of grepping blind. Use when searching for where a feature, param, DSP block, or UI panel lives in vxn-2.
---

# Find something in the vxn-2 source tree

A map of where things live, so a search starts narrow. Prefer the Explore agent or `Grep` scoped to the directory below that matches the topic, rather than grepping the whole repo. Verify before trusting — files move; this map is a starting guess, not ground truth.

## Workspace layout (`vxn-2/`)

```
crates/
  vxn2-dsp/       pure DSP, no host/state. Per-voice + per-stack signal path.
  vxn2-engine/    voice alloc, params/state, modulation, presets, master/FX glue.
  vxn2-clap/      CLAP plugin shell (host boundary, process callback entry).
  vxn2-ui-web/    HTML/JS faceplate (assets/) + Rust bridge.
  vxn2-app/       standalone app harness.
  vxn2-osc-bench/ oscillator benchmark harness.
xtask/            build/bundle/install automation (see [[vxn2-xtask-flat-workspace]]).
PARAMETERS.md     canonical param-table reference.
adrs/             vxn-2 design records (ADRs stay per-product under vxn-2/).
```

Work tracking lives in the **unified worklist at repo root**, not under `vxn-2/`:
`tickets/{open,closed}/` and `epics/{open,closed}/` cover both products; each item's
`product:` frontmatter field (`vxn-1`/`vxn-2`) says which synth it belongs to.

## DSP — `crates/vxn2-dsp/src/`

- Operators / FM: `op.rs`. Algorithms (DX7 routing): `algo.rs`. Voice: `voice.rs`. Stack (SoA alloc unit): `stack.rs` — see [[vxn2-stack-soa]].
- Envelopes / EG: `eg.rs`, `envelope.rs`. LFOs: `lfo.rs`. Keyboard scaling: `ks.rs`.
- Filter chain: `filter.rs`, `halfband.rs` (oversampling). See [[vxn2-filter-epic]].
- FX: `delay.rs`, `reverb.rs`. Sine: `sine.rs` + generated `sine_table.rs`.
- Helpers: `math.rs`, `smoother.rs`, `rng.rs`, `tables.rs`, `cleanup.rs`.

## Engine — `crates/vxn2-engine/src/`

- Voice allocation / polyphony: `alloc.rs`. Top-level engine + `reset`/`process`: `engine.rs`.
- Params + shared state (CLAP table, atomics, state blob): `params.rs`, `shared.rs`. Mod matrix: `matrix.rs`, `modulation.rs`.
- Presets: `preset.rs`, `preset_io.rs`, `factory.rs`, `default_patch.rs` — see [[vxn2-preset-system]]. Master bus / FX glue: `master.rs`. Tempo sync: `sync.rs`. Flush-to-zero: `ftz.rs`.

## UI — `crates/vxn2-ui-web/assets/`

- Entry: `index.html`, `main.js`, `bootstrap.js`, `style.css`.
- `panels/`: `op-row.js` (per-op controls + KS graph), `mod-matrix.js`, `algo-diagram.js`, `preset-bar.js`, `preset-browser.js`, plus widgets `fader.js` `knob.js` `button-group.js` `graph.js`. MVC discipline: view never reads model — see [[vxn2-mvc-discipline]].

## Search tips

- A **param** by name → grep `PARAMETERS.md` first for the canonical id, then `shared.rs`/`params.rs` for wiring, then the panel JS for the control.
- A **mod destination** → `matrix.rs` + `modulation.rs`.
- A **ticket/epic** by topic → `tickets/{open,closed}/` and `epics/{open,closed}/` are plain markdown; grep their titles.
- ARM NEON / vectorisation checks have a grep pitfall — see [[vxn1-neon-grep-pitfall]].
