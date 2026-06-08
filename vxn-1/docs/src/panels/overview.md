# Panel overview

The VXN1 faceplate groups every parameter into labelled panels arranged roughly in signal-flow order: sources at the top, filter and amp in the middle, modulation routes alongside, effects at the bottom. The headers below match the panel labels on the faceplate.

| Panel | Page |
| --- | --- |
| Oscillator 1 / Oscillator 2 | [Oscillators](oscillators.md) |
| Cross-Mod selector + amount | [Cross-modulation](cross-modulation.md) |
| Mixer (Osc 1 / Osc 2 / Sub / Noise) | [Mixer](mixer.md) |
| Filter (HPF + VCF) | [Filter](filter.md) |
| Env 1 / Env 2 | [Envelopes](envelopes.md) |
| LFO 1 / LFO 2 | [LFOs](lfos.md) |
| Pitch Mod / PWM Mod / Filter Mod / Mod Wheel | [Modulation routes](modulation.md) |
| Voice & assign | [Voice & assign](voice.md) |
| Phaser / Chorus / Delay / Reverb | [Effects](effects.md) |
| Master | [Master](master.md) |

## Per-layer vs. global

Every panel above except **LFO 2**, the **effects rack**, and **Master** is **per-layer** — you get a separate set of values for Upper and Lower. The faceplate shows one layer at a time; the **Layer switcher** in the header chooses which.

In **Whole mode**, only the Upper layer is visible — Lower mirrors Upper. In **Dual** and **Split** modes, both layers carry independent state and you can switch between them freely.

## Parameter conventions used in this manual

- **Range**: the raw value range the parameter accepts.
- **Default**: the value loaded by the **Init** patch.
- **Unit**: the natural unit the value is displayed in on the faceplate.
- **Taper**: how knob position maps to value. *Linear* is uniform; *exponential* is denser at the low end (good for time and frequency); *enum* is discrete steps.

## Reading the parameter tables

Each panel page closes with a parameter table:

| Parameter | Range | Default | Unit | Notes |
| --- | --- | --- | --- | --- |
| _Example_ | 0.001–10 | 0.005 | s | Exponential taper |

The full instrument-wide table is in the [Parameter reference](../parameter-reference.md).
