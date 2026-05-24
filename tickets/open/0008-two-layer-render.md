---
id: "0008"
title: Two-layer engine render
priority: high
created: 2026-05-25
epic: E003
---

## Summary

Make the engine render **two 8-channel layers**, each from its own per-patch
param block (0007), with its own `BlockCtx`, LFO, and modulation matrix, summed
into the global FX bus (ADR 0003 §1, §5, §10). This replaces the single
16-channel `VoiceBank` with two 8-channel layers and is the core DSP change.

## Acceptance criteria

- [x] Per-layer channel count = 8; two layers = 16 total (a new
      `CHANNELS_PER_LAYER = 8` const; `MAX_VOICES` total stays 16). Choose **two
      `VoiceBank`s of 8** or one bank processing 8-channel slices — whichever
      keeps the poly kernel's hoisted-global / vectorised lane loop intact
      *within* a homogeneous layer (ADR 0003 §10).
- [x] `build_ctx` becomes per-layer: each layer's `BlockCtx` is built from its
      own param block; each layer owns an LFO instance and resolves its own
      matrix.
- [x] `render` runs both layers and **sums** their outputs into the existing
      global chorus/delay bus (FX stays global — ADR 0003 §7).
- [x] **Whole-mode param source:** in Whole, both layers read **layer A's**
      param block (no mirroring); in Dual/Split each layer reads its own. The
      key-mode read is wired here even though event routing (Whole vs others)
      lands in 0009 — expose a clean `param_source(layer, key_mode)`.
- [x] Per-layer LFO/envelope/filter state resets correctly on
      `set_sample_rate` / `reset_all`.
- [x] Tests: with both layers fed identical params + notes, output equals a
      single-layer render of the same patch (Whole-equivalence); two different
      patches produce two distinguishable spectra summed; all 16 channels stay
      finite.

## Notes

- This ticket does **not** introduce event routing or assign modes — it renders
  both layers given (for now) the same note set, proving the two-pass structure
  and per-layer modulation. 0009 decides who hears which notes.
- The pitch-bend / mod-wheel values stay global but are applied per layer (each
  layer's `base_semis` + its own mod-wheel routing) — ADR 0003 §9.
- CPU: two passes; Whole is still 16 channels total so no worse than today.
- Depends on 0007. Validation: `cargo test -p vxn-dsp -p vxn-engine`.
