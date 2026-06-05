---
id: E001
title: VXN2 audio kernel
status: open
created: 2026-06-05
---

## Goal

Build the VXN2 audio kernel end-to-end: from a per-operator phase accumulator
through algorithm routing, voice and voice-stacking, modulation (LFOs +
envelopes + matrix), effects, and master output. The kernel ships as the
`vxn2-dsp` + `vxn2-engine` crates and is exercised by the existing
`vxn2-osc-bench` benchmarks plus new integration tests.

When this epic closes, the kernel can be driven by a stub host (note-on /
note-off + parameter sets) to produce audio that exercises every parameter
in `PARAMETERS.md`. The CLAP shell (`vxn2-clap`) and the production HTML
faceplate (`vxn2-ui-web`) are out of scope — separate epics — but the kernel
exposes the parameter-application surface they will sit on.

This epic does not block on UI implementation; the mockup at
`ui-mockup/index.html` is the layout reference. Parameter ranges and
defaults come from `PARAMETERS.md`.

## Scope

**In:**

- `vxn2-dsp`: framework-free DSP kernels — operator core, EG, key scaling,
  algorithm routing primitives, LFO, ADSR, FDN reverb, delay line, smoothing.
  Reuses `vxn-dsp` (VXN1) primitives where they fit (smoothers, BPM sync
  table, fixed-point phase types).
- `vxn2-engine`: voice allocator, voice-stack instantiator, mod matrix
  source/dest engine, FX chain, block render loop, parameter table.
- Benchmarks extending `vxn2-osc-bench`: full-voice cost, stacked-voice cost,
  matrix evaluation cost, FX chain cost.
- Integration test suite: render N seconds of audio for representative
  patches, compare RMS / spectral hashes against baselines.

**Out (later epics):**

- CLAP shell (`vxn2-clap`).
- Production HTML faceplate (`vxn2-ui-web`) and its panel JS.
- Preset format + factory bank.
- Algorithm editor (post-v1, ADR §12).
- Mod matrix condition fields (ADR §6, v2).
- Limiter, character FX, additional effects beyond clean delay + FDN reverb.
- MPE / per-note expression beyond channel-wide aftertouch.

## Tickets

- [ ] [0001 — Operator core (osc + EG + level + KS)](../../tickets/open/0001-operator-core.md)
- [ ] [0002 — Algorithm router (32 algos, carriers, modulators, feedback)](../../tickets/open/0002-algorithm-router.md)
- [ ] [0003 — Voice (6 ops + voice-level state, mono signal path)](../../tickets/open/0003-voice.md)
- [ ] [0004 — Polyphony allocator (16 voices, oldest steal)](../../tickets/open/0004-polyphony-allocator.md)
- [ ] [0005 — Voice stacking (density / detune / spread / phase / distrib)](../../tickets/open/0005-voice-stacking.md)
- [ ] [0006 — LFO1 global + LFO2 per-voice (delay + fade + key-sync)](../../tickets/open/0006-lfos.md)
- [ ] [0007 — Pitch EG + Mod Env](../../tickets/open/0007-extra-envelopes.md)
- [ ] [0008 — Mod matrix engine (sources, destinations, smoothing)](../../tickets/open/0008-mod-matrix.md)
- [ ] [0009 — Voicing modes (Whole / Layer / Split)](../../tickets/open/0009-voicing-modes.md)
- [ ] [0010 — Delay FX (BPM sync, ping-pong)](../../tickets/open/0010-delay-fx.md)
- [ ] [0011 — FDN reverb](../../tickets/open/0011-fdn-reverb.md)
- [ ] [0012 — Master out + parameter table assembly](../../tickets/open/0012-master-and-params.md)

## Dependency order

```text
0001 (operator core) ──> 0002 (algo router) ──> 0003 (voice) ──> 0004 (allocator) ──> 0005 (stacking)
                                                       │
                                                       └─> 0006 (LFOs) ──┐
                                                                          ├─> 0008 (matrix) ──> 0009 (voicing)
                                                       ┌─> 0007 (envs) ──┘
                                                       │
0010 (delay) ─┐
              ├─> 0012 (master + param table)
0011 (FDN)  ──┘
```

- 0001 is the foundation; benchmarks already exercise a precursor design.
- 0002 → 0003 → 0004 → 0005 is the synthesis stack; each layer composes the
  previous.
- 0006 + 0007 are modulation sources, evaluated per-voice or globally as
  appropriate. 0008 (matrix) consumes them as sources and applies to dests.
- 0009 builds on 0004 + 0008: split / layer are voice-allocator extensions
  that also reach into the matrix for per-layer routing.
- 0010 + 0011 are independent FX, summed into the chain at 0012.
- 0012 assembles the CLAP-facing parameter table referenced by every prior
  ticket and finalises the kernel surface.

## Acceptance

- The kernel renders a sustained note for each of the 32 algorithms without
  panics, allocations, or out-of-range output.
- A baseline stacked patch (density 4, detune 8 ct, spread 0.6) renders at
  real-time CPU cost ≤ a documented budget on the bench host (Apple M-series,
  44.1 kHz, 64-sample block) with 16 notes held — exact figure set during 0005.
- Modulation: every source/dest combination in `PARAMETERS.md` routes
  end-to-end through the matrix without DC offset or zipper noise.
- Per-voice LFO retrigger + delay + fade produce the expected onset envelope
  in offline rendering; sustained S&H produces decorrelated values across a
  stack when `voice_rand → lfo2_phase` is routed.
- FX bypass paths are bit-identical to the kernel output (no FX colouration
  when off).
- No RT allocations, no `unwrap`/`expect` in DSP, no panics across the
  process-callback boundary.
- Each ticket's individual acceptance criteria met.
