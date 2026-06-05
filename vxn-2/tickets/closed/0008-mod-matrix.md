---
id: "0008"
title: Mod matrix engine (sources, destinations, smoothing)
priority: high
created: 2026-06-05
epic: E001
---

## Summary

The mod matrix is VXN2's central modulation router. A fixed 16-slot table,
each slot binding a source to a destination with a depth + curve. Source
values are evaluated per control block; destinations are applied per sample
for pitch-shaped targets (zipper-sensitive) and per block for everything
else.

Per ADR §6 this is the *only* mechanism for dynamic parameter modulation;
hard-wired routes (like VXN1's mod-wheel → cutoff) don't exist.

## Acceptance criteria

- [ ] `MatrixSlot { source: SourceId, dest: DestId, depth: f32,
      curve: CurveKind }` POD, 16 of them in `MatrixTable`.
- [ ] `SourceId` enum covers all sources in `PARAMETERS.md` §"Mod matrix"
      (lfo1/lfo2/pitch_eg/mod_env/mod_wheel/aftertouch/velocity/key/
      voice_idx/voice_spread/voice_rand).
- [ ] `DestId` enum covers all destinations in `PARAMETERS.md`:
      - Per-op: ratio, level, detune, pan, feedback (× 6 ops = 30)
      - Global: pitch, lfo1_rate, lfo2_rate, lfo2_phase
      - Macros: stack_detune, stack_spread
      - FX: delay_mix, reverb_mix
- [ ] `CurveKind` enum: Lin, Exp, Log, Bipolar. Applied to source value
      before depth scaling.
- [ ] Per-block source evaluation: `eval_sources(modblock, voice_state)`
      populates a per-voice `[f32; N_SOURCES]` lookup table once per
      control block.
- [ ] Per-block destination application: for non-zipper-sensitive dests,
      accumulate `source_val × depth × curve` into a `[f32; N_DESTS]`
      accumulator, apply at block start.
- [ ] Per-sample destination application: for pitch-shaped dests
      (global_pitch, op_ratio, op_detune, lfo2_phase), apply smoothed
      modulation per sample. Use VXN1's smoothing primitive (one-pole IIR,
      time constant matching control block).
- [ ] Empty slots (source = SourceId::None) skip evaluation cheaply.
- [ ] Bench: `matrix_eval_full` (all 16 slots active) and `matrix_eval_empty`
      (all None) — empty case should be near-free.

## Notes

Smoothing strategy: VXN1's `vxn-dsp::smoother::OnePole` (or equivalent)
applied at the destination side, not the source side. Reason: a fast source
(e.g. S&H LFO) feeding a smoothed destination (pitch) gets the right
behaviour (the destination smooths the steps); a smoothed source feeding a
slow destination wastes smoothing work.

Don't make matrix slots automatable CLAP params in v1 (see PARAMETERS.md
caveat). Slot source/dest enums change patch-format, not automation surface.
Depths *are* matrix-routable (one slot can scale another's depth via the
matrix itself — needs care to avoid loops, which the v1 spec sidesteps by
having no cycle detection: a matrix slot's depth is a constant scalar in v1,
not a modulatable destination).

The matrix evaluation cost grows linearly with active slots. With 16 slots
and 128 voice instances, that's 16 × 128 source evaluations per block —
exactly what the SoA voice batch is built for. Vectorise across voice
instances per slot, not across slots per voice.
