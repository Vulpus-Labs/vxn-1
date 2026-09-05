---
id: "0361"
product: vxn-3
title: "vxn-3 hat source + filter rework — summed squares alongside XOR parity, steeper shaping, open/closed brightness split"
priority: medium
created: 2026-09-04
epic: E034
---

## Summary

Three related reasons the Metal family's hats never sound crisp.

**The metallic source is a ring modulator, not the 808's oscillator bank.** Metal takes the
sign-**parity** of six squares — `parity *= sgn` at
[metal.rs:314-319](../../vxn-3/crates/vxn3-engine/src/engines/metal.rs#L314-L319) — which is
a product, giving a much sparser and harsher spectrum than the 808's **sum** of its six
square oscillators. XOR-parity is a well-known cheap trick, but it is a different sound, and
it is the only metallic source on offer.

**The filter is 6 dB/octave.** The whole shaping stage is one one-pole highpass
(`hp_coef` cooked at
[metal.rs:277](../../vxn-3/crates/vxn3-engine/src/engines/metal.rs#L277)). The 808 hat runs
a highpass *and* a bandpass. One pole cannot separate "bright" from "hissy", so the flavour
ends up dull or thin with nothing in between.

**Open and closed are spectrally identical.** `Closed Hat` and `Open Hat` are literally the
same flavour function ([metal.rs:120-128](../../vxn-3/crates/vxn3-engine/src/engines/metal.rs#L120-L128)),
differing only in decay via the note split. A real open hat is *brighter* as well as longer.

While in this code, fix the nested crossfade at
[metal.rs:338-339](../../vxn-3/crates/vxn3-engine/src/engines/metal.rs#L338-L339) —
`modal * (1-mix) + hy * mix`, then `metal_sum * (1-noise) + nhy * noise` — which caps total
energy and forced the hand-tuned `× 2.0` compensation at
[metal.rs:332](../../vxn-3/crates/vxn3-engine/src/engines/metal.rs#L332).

## Design

- **`Source`** (`Ratio`, 0..1 rounded to an index, default **0** = XOR parity) — selects
  between parity and a **summed**-squares mode over the same six `XOR_RATIOS` oscillators.
  Sum mode needs a `1/6` normalisation to sit at a comparable level. Default keeps every
  existing flavour bit-for-bit.
- **Two-stage shaping.** Add a resonant bandpass ahead of the existing highpass on the
  metallic path — the Noise family already has a TPT-SVF worth lifting
  ([noise.rs:245-254](../../vxn-3/crates/vxn3-engine/src/engines/noise.rs#L245-L254)); move
  it into `vxn3-dsp` as a shared primitive rather than copying it. New params **`Band`**
  (`Hertz`) and **`Band Q`** (`Ratio`), defaulting to a wide/bypassed setting.
- **Open/closed brightness split.** Add **`Open Bright`** (`Hertz`, default = same as
  `Bright`, so no change) and select it on trig alongside the open decay, the same way
  `cur_decay` is chosen at
  [metal.rs:352-358](../../vxn-3/crates/vxn3-engine/src/engines/metal.rs#L352-L358).
- **Independent layer levels** for modal / metallic / noise instead of the nested
  crossfades, and drop the `× 2.0` fudge once the levels are honest.
- **Re-author** `flavour_hat`, `flavour_ride`, `flavour_crash`, and split Closed Hat and
  Open Hat into two flavours now that they can differ by more than decay.

## Acceptance criteria

- [ ] At the new params' defaults, every authored Metal flavour renders bit-for-bit
      identical to the pre-ticket engine.
- [ ] A test proves sum mode differs from parity mode and is spectrally denser (HF-fraction
      or zero-crossing count against the parity source at the same body pitch).
- [ ] A test proves the bandpass shapes the metallic path (mirror
      `bandpass_shapes_noise_colour`).
- [ ] A test proves an open hit is brighter than a closed hit on the same flavour when
      `Open Bright` differs — HF-fraction comparison across the note split.
- [ ] Modal, metallic and noise layers can each reach full level without attenuating the
      others; the `× 2.0` compensation is gone.
- [ ] The SVF is a shared `vxn3-dsp` primitive used by both `Noise` and `Metal`, not
      duplicated.
- [ ] `metal_flavours()` re-authored with distinct Closed and Open Hat entries; pairwise
      distinctness test still passes.
- [ ] Round-trip + truncated-patch tolerance updated for the new `METAL_P`.
- [ ] `cargo test -p vxn3-engine -p vxn3-clap` green; clippy clean; alloc-trap passes.

## Notes

- Coordinate with **0357**, which also touches `Metal::cook` and the decay selection — that
  ticket adds per-mode damping arrays and a second cooked array for the closed case. Land
  0357 first; the brightness split here follows the same open/closed select it establishes.
- The faceplate's `NOTE_BY_VOICE` map in
  [app.js](../../vxn-3/crates/vxn3-ui-web/assets/app.js) keys Closed Hat at note 38 and Open
  Hat at 50 against the split at 44 — splitting them into two flavours must keep that
  relationship and the choke group intact.
- Out of scope: a strike transient for the hat (there is none today beyond the envelope
  step). Assess after the filter rework — the bandpass may supply the bite on its own.
