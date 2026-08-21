---
id: "0269"
product: vxn-1b
title: "LFO 1 Rate as a mod destination — per-voice, multiplicative on the resolved Hz"
priority: medium
created: 2026-08-21
epic: E039
depends: ["0268"]
---

## Summary

`LFO 1 Rate` becomes a mod-matrix destination: a per-voice multiplier of
0.25× .. 4× (±2 octaves, unity at centre) on the rate LFO 1 is already running
at. ADR 0001 §2 listed "LFO rate" as a candidate dest; this ships it for LFO 1
only.

**LFO 1 only.** LFO 1 is per-voice (`[LfoCore; N]` per bank), so a rate route is
an ordinary per-voice dest. LFO 2 is a single synth-global oscillator broadcast
to every voice; a per-voice dest accumulator has nothing coherent to say about
its rate (which voice would win?), and the per-voice sources — velocity, key,
the envelopes — are meaningless there. Left out rather than fudged.

**Multiplicative on the resolved Hz**, i.e. applied *after* tempo sync (0267)
has turned a synced fader position into a rate. `2^x` mapping, so any
power-of-two amount moves a synced LFO between subdivisions and stays on the
grid; other amounts land between them, exactly as the Rate fader does when Sync
is off.

**One control block of lag.** LFO 1 is itself a matrix *source*, and the bank
ticks all lanes' LFOs before evaluating the matrix — a same-block read would be
circular. The dest total is therefore carried over from the previous control
block (32 samples, ~0.7 ms at 48 kHz), which no realistic source cares about.
A note-on re-rates its lane immediately from that block's own total, so a
stolen voice never runs even one block at the previous note's rate.

## Acceptance criteria

- [ ] `DestId::Lfo1Rate` (wire `lfo1-rate`, label `LFO 1 Rate`), `N_DESTS`
      13 → 14, `DEST_GAIN` 2.0 = ±2 octaves of rate.
- [ ] `eval::lfo_rate_scale`: total → `2^clamp(x, −2, 2)`, rails at 0.25× / 4×,
      unity at 0.
- [ ] `RenderBank` keeps a per-lane `lfo1_rate_mod` carried over one block; the
      per-lane `set_rate` is `ctx.lfo1_rate_hz × lfo_rate_scale(mod)`.
- [ ] A note-on re-rates its own lane from the trigger block's total.
- [ ] No route → every lane at exactly the panel Hz, render bit-identical.
- [ ] Tests: the ×4 / ×0.25 rails behaviourally (phase advance per block), a
      synced rate multiplying to another subdivision's rate exactly, stolen-lane
      re-rate, and the unrouted case.

## Notes

`LfoCore::set_rate` clamps to 0.001..40 Hz, so a 4× route on a fast panel rate
tops out there rather than aliasing the control-rate LFO.

Deliberately continuous, unlike the envelope time scales of 0268: a phase
increment can change mid-note without artefacts, so there is no reason to latch
it at note-on.
