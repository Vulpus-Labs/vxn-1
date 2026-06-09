# ADR 0002 — Drop dual-layer voicing (Whole-only)

- **Status:** Accepted
- **Date:** 2026-06-09
- **Scope:** Reverses ADR 0001 §8. Removes Voicing-mode (Whole / Layer /
  Split), the Upper/Lower parameter pair, and all per-layer infrastructure.
- **Supersedes:** ADR 0001 §8 (Voicing modes), and the closed ticket
  [E001 / 0009 — Voicing modes](../tickets/closed/0009-voicing-modes.md).

## Context

ADR 0001 §8 imported the two-layer mechanism from VXN1 (Jupiter-8 lineage),
keeping it for two performance idioms:

- **Layer** — two parallel patches summed under one note (pad + chime,
  bass + lead).
- **Split** — keyboard-split patches (mono bass left of the split,
  poly lead right of it).

Both modes carry significant structural cost: every per-layer parameter
exists twice in the CLAP table (162 × 2 = 324 IDs), the engine carries
`PatchMatrix.upper` + `.lower` matrix tables, the allocator carries a
`[Layer; N_STACKS]` tag array, the matrix evaluator demuxes routings per
held stack by its captured layer, the UI carries an Upper/Lower edit
selector and a voicing-mode picker, and the JS panel layer threads
`editLayer` through every param lookup. Per the closed-ticket inventory
this is ~800–1000 lines of Rust + ~300–400 lines of JS.

The justification was sound design — two contrasting tones per note. But
that justification was inherited from a subtractive synth where one voice
is a low-pass filter on a saw/pulse. In subtractive, every voice has the
same overall character; getting two characters at once *requires* two
voices summed.

## Decision

Drop dual-layer entirely. A patch is one parameter set. Voicing-mode is
removed. Split-point is removed. The `Upper/Lower` edit toggle is removed.
Every `upper-*` / `lower-*` CLAP ID collapses to a single unprefixed ID.

The operator-based engine — 6 ops × 32 algorithm graphs × per-op EG and
KS — already supplies the sound-design surface that Layer mode was filling.
A pad-swell + upper-chime is one patch: pick algo 8 (two independent
3-op stacks) or 22 (one carrier through a modulator chain plus a free
carrier), give the "pad" carriers slow attack EGs and low ratios, give the
"chime" carrier a percussive EG and high ratio, mod-matrix-gate the chime
on velocity. The combinatorial space of {algo × per-op ratio × per-op EG ×
per-op KS × per-op level × per-op feedback × LFO2 per-voice × pitch EG ×
mod env × matrix} is wide enough to express what Layer mode expressed
with two parallel patches, and to express things Layer mode could not
(intermodulation between the two halves).

Split is not synth territory — DAWs already provide keyboard-range
splits at the MIDI level (Logic's Track Stack, Ableton's external
instrument range, FL's MIDI Out filters, Bitwig's note FX range). A
synth-internal split duplicates host functionality. If a user wants
split-key behaviour, the host is the right place; if the user wants
note-range-dependent timbre within one patch, the mod matrix already
exposes `key` as a source with a curve.

## Consequences

### Removed (engine)

- `vxn2_engine::voicing` module (the whole file): `VoicingMode`,
  `VoicingParams`, `LayerParams`, `Patch::split_layer`, `Patch::layer`.
- `Patch.upper` / `Patch.lower` fields. `Patch` becomes a single
  parameter set (renamed `Patch` → flat `Patch { ... }` holding what was
  previously `LayerParams`).
- `matrix::PatchMatrix` (upper + lower tables). The engine carries a
  single `MatrixTable`.
- `matrix::Layer` enum.
- `PolyAlloc::layers: [Layer; N_STACKS]` + `stack_layer()` accessor.
- Layer-aware dispatch in `PolyAlloc::note_on_patch` and matrix
  evaluation in `Engine::process_block`.

### Removed (params)

- All `upper-` / `lower-` ID prefixes. Per-layer params reduce from
  162 × 2 = 324 CLAP IDs to 162.
- `voicing-mode` CLAP param.
- `split-point` CLAP param.
- `mtx_depths: [[f32; 8]; 2]` collapses to `mtx_depths: [f32; 8]`.
- The `op_block_arr!` / `per_layer_rest_arr!` macros (replaced with a
  single unprefixed-id macro).
- `N_LAYERS = 2` → constant disappears.
- Total CLAP-exposed params: **345 → 180** (163 per-patch collapsed +
  17 patch-level after dropping voicing-mode and split-point).

### Removed (CLAP shell)

- All `upper-` / `lower-` ID lookups in `vxn2-clap/src/{lib.rs,local.rs}`.
- The shell stops demuxing param writes through a layer prefix.

### Removed (app + UI)

- `vxn2_app::model::Layer` enum and every method that takes a `Layer`.
- `events::{SetEditLayer, EditLayerChanged}` and the `layer` field on
  `SetOpTab` / `OpTabChanged` / `MatrixSnapshot`.
- `vxn2-ui-web/assets/index.html` voicing-mode button group and
  edit-layer toggle.
- `assets/main.js` `editLayer` state machine and `upper-` / `lower-`
  ID prefixing in panel binding.
- `assets/panels/mod-matrix.js` per-layer routing and `onEditLayerChanged`.
- `assets/panels/op-row.js` `currentLayer()` method.
- `assets/style.css` `.edit-layer-toggle.muted` rule.
- `ui-mockup/index.html` voicing-mode group (mockup is layout-reference
  source-of-truth per ADR 0001 §11).

### Kept

- Mod matrix as the single dynamic-routing engine (ADR 0001 §6) —
  unchanged shape, just one table per patch instead of two.
- Stacking macros (ADR 0001 §3) — unchanged. Stacking is per-patch, not
  per-layer.
- LFO1 / LFO2 (ADR 0001 §4), Pitch EG, Mod Env (§5), FX chain (§7),
  Master (§11) — all already patch-level or single-layer-trivial.
- The 32 DX7 algorithms (§2) and per-op feedback extension (§1) —
  these *are* the sound-design surface that replaces Layer.

### Migration

VXN2 is pre-release. No deployed patches exist. The factory bank is not
yet authored. No migration shim is needed; the param table simply
rebuilds at its new (smaller) shape.

### Performance

CPU cost is unchanged in single-layer use (Whole was already the active
fast path). The work that Layer mode would have doubled (two stacks per
note) is now never doubled; a user who wants the equivalent CPU spend
authors a single patch with a higher `stack_density` instead. This is
strictly cheaper than dual-layer at the same density (one allocator
slot per note, not two; one matrix evaluation per stack, not one per
layer).

### Documentation impact

- ADR 0001 §8 is superseded; a forward-note added on revisit.
- `PARAMETERS.md` rewritten: "Scope: per-layer vs patch-level" section
  deleted, every `(per-layer)` tag dropped, totals recomputed.
- Closed ticket 0009 marked superseded.
- README.md mentions of layered voicing removed.

## Open questions revisited

- *Will users miss split-key?* Hosts cover it. No internal need.
- *Will users miss layered timbres?* The operator surface is the
  answer; if users surface specific patches that can't be expressed in
  a single voice, that's evidence for a feature ticket, not for
  resurrecting two-layer infrastructure.
- *Should the matrix carry a `key`-curve helper?* Already does
  (`source = key`, `curve = lin/exp/log/bipolar`) per ADR 0001 §6.
  Documenting it as the recommended split-replacement is a docs task,
  not a code task.

## Tickets

Tracked under [E004 — Single-layer collapse](../epics/open/E004-single-layer-collapse.md).
