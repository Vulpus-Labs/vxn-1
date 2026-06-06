---
id: "0008"
title: Mod matrix engine (sources, destinations, smoothing)
priority: high
created: 2026-06-05
epic: E001
---

## Summary

The mod matrix is VXN2's central modulation router. A fixed 16-slot table
**per layer** (Upper + Lower), each slot binding a source to a destination
with a depth + curve. Source values are evaluated per control block;
destinations are applied per sample for pitch-shaped targets
(zipper-sensitive) and per block for everything else.

Per ADR §6 this is the *only* mechanism for dynamic parameter modulation;
hard-wired routes (like VXN1's mod-wheel → cutoff) don't exist.

Per ticket 0009 the matrix is per-layer; this ticket sizes for two tables
(Whole mode uses Upper only, Layer/Split mode uses both).

## Acceptance criteria

- [x] `MatrixSlot { source: SourceId, dest: DestId, depth: f32,
      curve: CurveKind }` POD, 16 of them in `MatrixTable`. Patch holds two
      `MatrixTable`s (Upper + Lower) via `PatchMatrix`.
- [x] Slots **1–8** `depth` per layer flagged for CLAP exposure via
      `N_CLAP_DEPTH_SLOTS = 8`. CLAP param wire-up itself lands with 0012
      (Master + Params); the matrix module documents identifiers + the
      automatable subset.
- [x] `SourceId` enum covers all sources in `PARAMETERS.md` §"Mod matrix"
      (lfo1/lfo2/pitch_eg/mod_env/mod_wheel/aftertouch/velocity/key/
      voice_idx/voice_spread/voice_rand) + `None` sentinel.
- [x] `DestId` enum covers all destinations in `PARAMETERS.md`:
      - Per-op: ratio, level, detune, pan, feedback (× 6 ops = 30)
      - Global: pitch, lfo1_rate, lfo2_rate, lfo2_phase
      - Macros: stack_detune, stack_spread
      - FX: delay_mix, reverb_mix
      Plus `None` sentinel. `is_pitch_shaped()` flags the 14-entry
      zipper-sensitive subset.
- [x] `CurveKind` enum: Lin, Exp, Log, Bipolar. Applied per slot inside
      `eval_dests` (hoisted out of the per-lane loop so the inner pass
      autovectorises).
- [x] Per-block source evaluation: `eval_sources(patch, stack, lanes, out)`
      fans patch-global + per-stack + per-lane sources into a per-lane
      `[[f32; N_SOURCES]; STACK_LANES]` lookup, once per control block.
- [x] Per-block destination application: `eval_dests` walks slots,
      accumulates `source × curve × depth` into `[[f32; N_DESTS]; STACK_LANES]`
      per stack. Caller applies the accumulator at block start.
- [x] Per-sample destination application: `PitchSmoother` holds a one-pole
      IIR per pitch-shaped dest per lane (14 × 8 = 112 cells). Time
      constant matches the control block via VXN1's
      `vxn2_dsp::smoother::one_pole_coeff` (ported into vxn2-dsp).
- [x] Empty slots (`SourceId::None`, `DestId::None`, or `depth == 0.0`)
      short-circuit via `Option::idx` `let-else` at the slot head — no
      per-lane work for empty rows.
- [x] Bench: `vxn2-osc-bench/benches/matrix.rs` — `matrix_eval_full` (all
      16 slots active across every curve kind, distinct source/dest per
      slot) and `matrix_eval_empty` (all `None`). On M-series at this
      writing: full ≈ 70 ns, empty ≈ 24 ns per iter — empty dominated by
      the per-lane dest clear + source-table broadcast, ≈ 3× under full.

## Notes

Smoothing strategy: VXN1's `vxn-dsp::smoother::OnePole` (or equivalent)
applied at the destination side, not the source side. Reason: a fast source
(e.g. S&H LFO) feeding a smoothed destination (pitch) gets the right
behaviour (the destination smooths the steps); a smoothed source feeding a
slow destination wastes smoothing work.

Slot `source` / `dest` / `curve` are never CLAP-automatable — they're
topology selectors and changing them mid-stream rewires routing rather than
sweeping a continuous control. Slot `depth` is the modulatable quantity:
slots 1–8 expose depth as CLAP params per layer (compromise — enough for
expressive macros, avoids 32 depth params bloating the table); slots 9–16
depth is patch state only. Users park DAW-driven routings in the low-index
slots; UI flags slots 1–8 as automatable.

Matrix-routing a slot's depth (one matrix slot scaling another's depth via
the matrix itself) is **not** supported in v1: depths are constant scalars
per block, not matrix destinations. Sidesteps cycle detection. The CLAP
exposure of slots 1–8 doesn't change this — DAW automation writes the depth
value, but the matrix engine still treats it as a constant for the block.

The matrix evaluation cost grows linearly with active slots. With 16 slots
and 128 voice instances, that's 16 × 128 source evaluations per block (per
layer) — exactly what the SoA voice batch is built for. Vectorise across
voice instances per slot, not across slots per voice. In Layer/Split mode
this doubles to two passes (Upper then Lower); each voice belongs to one
layer so they don't cross-pollinate.
