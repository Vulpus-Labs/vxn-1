---
id: E021
product: vxn-3
title: "vxn-3 MVP — synthesis drum machine groove proof"
status: open
created: 2026-06-15
---

> **First vxn-3 epic.** Design is fixed in `vxn-3/adrs/0001` (overall) and
> `vxn-3/adrs/0002` (FX). This epic is the *MVP cut* — the smallest build that
> proves the thesis end-to-end in a DAW and deliberately defers breadth. The
> cut line: if it doesn't make a hypnotic 8-track minimal-techno loop with a
> dub delay throw, it's not MVP; everything past that is breadth.

## Goal

A loadable CLAP plugin that proves vxn-3's thesis — **sample-free synthesis,
pitched as readily as percussive, with accessible *interesting* rhythm** —
playable in a host. The thesis lives in two places, so both are in scope:

- **The pattern engine** (the differentiator): polymeter, probabilistic trigs,
  retrig n-over-m. Nearly free — sequencer logic, no DSP.
- **The engine-defined voicing model** (the load-bearing architectural novelty,
  ADR 0001 §4/§5): one poly engine + one resonator engine over a shared per-track
  SoA block, validating lanes-as-voices vs lanes-as-modes.

FX, breadth of engines, and presets are *not* the thesis and are cut to a
single delay send + master limiter.

When this epic closes:

- vxn-3 loads as a CLAP plugin, syncs to host transport, and renders audio.
- 8 tracks, each running one of three engines (`Kick/Tone`, `Metal`, `Noise`).
- A pattern with per-track length (polymeter), per-trig probability, and
  retrig n-over-m plays sample-accurately to the host clock.
- Step p-locks (revert + latch) drive per-hit variation and the dub throw.
- A delay send bus (p-lockable send = the throw) + terminal master limiter.
- A minimal HTML faceplate to program and play it.

## Why this cut

The riskiest, most novel part is the engine trait with engine-defined voicing,
trigger, and choke semantics over a uniform SoA block. One poly + one resonator
engine de-risks it; three engines make a credible minimal-techno kit (kick/tom/
bass-stab from `Kick/Tone`, hats/ride from `Metal`, snare/clap from `Noise`).
The pattern levers are cheap and *are* the product, so they stay. Everything in
ADR 0002 beyond one delay bus, plus ramp/curve p-locks, conditional trig groups,
variable SoA width, presets, and the patches-graph engine, is pure breadth and
deferred.

## Scope

**In:**

- CLAP shell reusing `vxn-core-*`, host transport/tempo sync, stereo out.
- Track model + `Engine` trait (ADR 0001 §4/§5): off-thread engine swap,
  per-block dispatch, per-track 4-wide SoA block.
- Three engines: `Kick/Tone` (poly), `Metal` (modal resonator), `Noise`.
- Pattern engine: per-track step grid + length (polymeter), per-trig
  probability, retrig n-over-m, lane-local tick base.
- p-locks: step shape only — revert (hold N) + latch — on track params + send
  amount (ADR 0001 §3a subset).
- One delay send bus + terminal master limiter (ADR 0002 subset).
- Minimal HTML faceplate: grid, per-track engine select + knobs, playhead.

**Out (deferred, post-MVP):**

- Full ADR 0002 FX: 9-module roster, inserts, 4 buses, external CLAP
  send/return, compressor/EQ/gate/phaser/reverb/bitcrush/distortion.
- Ramp/curve p-lock behaviours; conditional trig groups; variable SoA width >4.
- Bus/master FX param automation; preset system; patches-graph engine.

## Planned tickets

- [ ] 0046 — CLAP shell + crate skeleton + host transport sync (silent).
- [ ] 0047 — Track model + `Engine` trait + `Kick/Tone` poly engine + basic
      step grid + audio out (hear one track).
- [ ] 0048 — Pattern engine: polymeter, probability, retrig n-over-m.
- [ ] 0049 — `Metal` (resonator) + `Noise` engines (validate voicing split).
- [ ] 0050 — Step p-locks (revert + latch) on params + send amount.
- [ ] 0051 — Delay send bus (p-lockable = dub throw) + master limiter.
- [ ] 0052 — Minimal HTML faceplate.

## Risks

- **Engine trait generality.** The poly-vs-resonator abstraction over one SoA
  block is unproven; 0047+0049 are the de-risk. If the trait fights the SoA
  codegen (cf. vxn-1/vxn-2 match-in-lane-loop lesson), it surfaces here.
- **Sample-accurate polymeter + retrig** against the host clock at block
  boundaries — trig scheduling must be exact, not block-quantised.
- **Click-free off-thread engine swap** while the sequencer runs.
- **Limiter latency / PDC** must be reported so the host compensates.

## Acceptance

- vxn-3 loads in a CLAP host, syncs transport, renders a hypnotic 8-track
  minimal-techno loop using all three engines.
- Polymeter tracks phase; probability thins; retrig fires n-over-m on the
  lane-local grid — all sample-accurate.
- A step p-lock on a delay send throws a hit into the delay tail; latch holds a
  param until the next lock; master limiter catches peaks and reports latency.
- Process callback is allocation-free; engine swaps don't click.
- The faceplate can program a pattern, pick engines, and tweak knobs end-to-end.
