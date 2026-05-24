---
id: E001
title: Foundational quick wins
status: done
created: 2026-05-24
closed: 2026-05-24
---

## Goal

Land the three cheapest, most isolated features from the post-v1 roadmap
(ADR 0002): a pre-VCF **high-pass filter**, explicit **oscillator octave
controls**, and **LFO delay / fade-in**. None of them touch the structural
hard parts of the roadmap — they add no oscillator coupling (unlike sync /
cross-mod), no second patch layer (unlike key modes), no transport dependency
(unlike LFO host-sync), and no change to the modulation-matrix dimensions
(unlike the second LFO). Each is additive: new params appended at the end of
the table (stable CLAP ids), localised DSP, and a self-contained voice-bank
change.

This epic exists to build momentum and exercise the "add a feature" path
(param table → engine ctx → voice DSP → tests, and eventually UI) on low-risk
work before the heavier items.

## Scope

**In:**

- `vxn-dsp`: a one-pole (−6 dB/oct) high-pass filter kernel + 16-voice SoA
  variant, matching the `PolyLadder` pattern.
- HPF wired per-voice into the signal path between the source mix and the
  ladder VCF (the JP-8 topology: Mixer → HPF → VCF → VCA).
- Per-oscillator octave control (separate from the existing coarse-semitone and
  fine-cent controls), preserving the ability to set non-octave intervals.
- LFO delay: a per-voice fade-in of LFO modulation depth after note-on.
- Params, engine `BlockCtx` plumbing, and unit tests for all three.

**Out (deferred):**

- Editor / faceplate controls for the new params — the engine + param model is
  the deliverable here; UI placement is a follow-up (can ride the next UI pass).
- Per-LFO scope for the delay (there is only one LFO until the second-LFO
  ticket in a later epic); revisit then.
- Hard sync, cross-mod, unison, portamento, env time-scaling, key modes — all
  separate, larger epics.

## Tickets

- [x] [0001 — High-pass filter (vxn-dsp + engine)](../../tickets/closed/0001-hpf.md)
- [x] [0002 — Oscillator octave controls](../../tickets/closed/0002-osc-octave.md)
- [x] [0003 — LFO delay / fade-in](../../tickets/closed/0003-lfo-delay.md)

## Dependency order

```text
0001 (HPF) ─┐
0002 (Octave) ─┼─ independent; any order, can be parallel
0003 (LFO delay) ─┘
```

All three are independent. 0001 is the most substantial (new kernel); 0002 is
the most trivial.

## Acceptance

- A patch can high-pass-filter the voice below the VCF, audibly thinning body.
- Each oscillator has an octave control that stacks with coarse/fine, and
  non-octave intervals (e.g. a fifth) remain expressible.
- LFO modulation fades in over a settable delay after each note-on.
- New params have stable CLAP ids (appended at the end of the table), defaults
  in range, and the existing param-table tests still pass.
- No RT allocation added to the audio path; no `unwrap`/`expect` in DSP.
