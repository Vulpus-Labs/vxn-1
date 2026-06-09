---
id: "0009"
title: Voicing modes (Whole / Layer / Split)
priority: medium
created: 2026-06-05
epic: E001
---

> **SUPERSEDED** by [ADR 0002 — Drop dual-layer voicing](../../adrs/0002-drop-dual-layer.md)
> and [E004 — Single-layer collapse](../../epics/open/E004-single-layer-collapse.md).
> Voicing-mode infrastructure has been removed. The acceptance criteria
> below describe a feature that no longer ships; the body is retained as a
> historical record.

## Summary

Per ADR §8: a patch can be one of three voicing modes.

- **Whole**: one patch parameter set drives all voices.
- **Layer**: two parameter sets (Upper / Lower), both triggered by every
  note, summed at the output. Doubles voice count per note.
- **Split**: two parameter sets, Upper triggered by notes ≥ `split_point`,
  Lower triggered below. Voice count per note unchanged.

Inherits VXN1's two-layer infrastructure (`vxn_app::Layer` model). The
op-detail UI panel's `edit_layer` toggle selects which layer is being edited
in Layer / Split modes.

## Acceptance criteria

- [x] `Patch` holds either one parameter set (Whole) or two (Layer / Split).
      Use the same `PatchParams` struct shape, just held singly or in pairs.
- [x] `note_on(note, vel)` dispatches:
      - Whole: one allocation per note
      - Layer: two allocations per note (one per layer set), both gated
        identically
      - Split: one allocation per note, layer chosen by note vs split_point
- [x] Voice cap (16 voices) applies to the *total* in-flight voices across
      layers. In Layer mode, polyphony is effectively halved.
- [x] `split_point` is a MIDI note (0..127). Notes equal to or above the
      split go to Upper; below go to Lower.
- [x] Mod matrix slots are per-layer: the Upper and Lower layers each have
      their own 16-slot matrix.
- [x] FX (delay, reverb) are *patch-level*, not per-layer — both layers feed
      the same FX chain.
- [x] LFOs: LFO1 is patch-level (shared). LFO2 is per-voice (so each layer's
      voices have their own LFO2 instances).
- [x] Mode change in the middle of playback: existing voices play out;
      new note-ons honour the new mode.
- [x] Test: Layer mode with two contrasting patches plays both on every
      note. Split mode with split_point=60 plays Upper on C4 and above,
      Lower on B3 and below.

## Notes

The `edit_layer` UI toggle is non-automatable view state, not engine state.
It just controls which of the two parameter sets the host's parameter writes
hit. The engine sees Upper and Lower as two independent `PatchParams`
references; the CLAP shell will multiplex parameter writes through the
selected layer.

VXN1's `vxn_app::layer` module is the reference. The biggest engine change
from Whole→Layer is doubling the allocator's voice array references — the
allocator itself doesn't change, it just gets called twice per note in Layer
mode. Cleanest implementation: a `LayerDispatch` wrapper around two
allocators.

Don't ship cross-layer parameter linking (one knob controls both layers) in
v1. If users ask, it's a UI macro layer over the per-layer params, not an
engine feature.
