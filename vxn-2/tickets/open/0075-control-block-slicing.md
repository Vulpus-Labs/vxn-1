---
id: "0075"
title: "Slice host buffers into CONTROL_BLOCK chunks — zipper persists at large buffers"
priority: high
created: 2026-06-10
epic: E006
depends: ["0074"]
---

## Summary

Fourteenth ticket of [E006](../../epics/open/E006-review-remediation.md).
The 0074 manual listen still hears zipper on LFO → OpNLevel / OpNPan.
The engine-side ramp is correct (verified by a block-edge
second-difference probe: edge ≈ interior at 32-sample blocks), but the
CLAP shell renders each event batch in **one** `Engine::process_block`
call ([lib.rs](../../crates/vxn2-clap/src/lib.rs) process loop), so
the control rate — LFO sampling, matrix eval, EG ticks — degrades to
the host buffer rate. The `CONTROL_BLOCK = 32` const documents the
intended slicing ("the audio-thread loop in 0016 will slice host
buffers into chunks of at most this size") but 0016 never implemented
it and its ACs never mentioned it.

## Why the ramp alone can't fix this

The 0074 linear ramp interpolates between block-rate LFO samples; the
residual is the linear-interpolation error of the LFO curve, which
scales with the **square** of the block length (`|f''|·h²/8`). At a
48 kHz / 512-sample host buffer with a 5 Hz full-depth sine LFO that
is ~1.4 % AM ripple at 93.75 Hz — clearly audible buzz; at 1024 it is
~5.7 %. At a 32-sample control block the same error is 5.5e-5
(≈ −85 dB) — inaudible. Measured sidebands around a C4 carrier confirm
the trend (≈ −74 dBc at 32-sample blocks vs ≈ −47 dBc at 1024).

Everything block-rate degrades the same way with big host buffers: EG
attack quantisation, pitch-smoother target refresh, LFO2, FX-mix mod.
Fixed-size slicing fixes the whole class.

## Design

In `VxnAudioProcessor::process`, inside the event-batch loop, render
each batch's `[start, end)` range in chunks of at most `CONTROL_BLOCK`
samples instead of one `process_block` call. `dt` is derived from `n`
inside the engine, so envelopes/LFOs tick correctly without other
changes. Event timing is unaffected (batches already split at event
boundaries; slicing only subdivides between them).

Cost: block-rate work (matrix eval, `apply_pitch_mult` powf, EG ticks)
runs per 32 samples instead of per host buffer — this is the engine's
designed operating point (`Engine::new(sr, CONTROL_BLOCK)`, same model
as VXN1) and what every engine bench already measures. FX
`set_params` re-cook when an FX-mix route is active also moves to
32-sample rate; acceptable, noted here in case it shows in profiles.

## Acceptance criteria

- [x] `process` never hands `Engine::process_block` a slice longer
  than `CONTROL_BLOCK`; tail chunks may be shorter.
- [x] Chunking helper unit-tested (full range, ragged tail, empty
  batch range).
- [x] Engine-level zipper regression test: LFO1→Op1Level and
  LFO1→Op1Pan at 32-sample blocks keep block-edge |d²| within 1.5× of
  block-interior |d²| over a 1 s render (guards the 0074 ramp with an
  audio-domain assertion, which the state-convergence tests don't).
- [ ] Manual listen (carried over from 0074): slow (~0.5 Hz) and fast
  (~8 Hz) LFO on level and pan, host buffers 64 and 512 — smooth at
  both.

## Close-out (2026-06-10)

Implemented as designed: `control_chunks` iterator in the CLAP shell,
batch ranges subdivided to ≤ 32 samples. The engine needed no changes.

Measurements (probe renders, LFO1 → Op1Level at 5 Hz full depth, C4):

- Engine driven at host-buffer rate (pre-fix shell behaviour): block-
  edge/interior |d²| ratio ≈ 3.2 at 512-sample buffers; block-rate
  sidebands ≈ −47…−57 dBc at 512–1024. Driven at the 32-sample control
  rate (post-fix behaviour for every host buffer size): ratio ≈ 1.08,
  sidebands ≈ −74 dBc — the linear-interp scalloping floor.
- Cost (M1, release, engine-level timing): 16 notes × density 8 with
  the level route active goes 5.6 % → 6.3 % RT (32 vs 256-sample
  blocks, +13 % relative); single note unchanged (0.85 % RT). The
  `master_chain` bench is engine-internal at a fixed block size and is
  unaffected by this shell change.
