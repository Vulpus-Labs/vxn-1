# Filter

VXN1's filter section is a two-stage chain:

1. **High-pass filter** (HPF) — 1-pole, pre-VCF. Used to trim low-end ahead of the main filter; great for keeping pads from muddying low registers.
2. **OTA-C ladder VCF** — 4-pole transistor-ladder model (R3109 / IR3109-flavoured) with selectable LP / HP / BP / Notch outputs and a 12 / 24 dB/oct slope switch.

## High-pass filter

Single knob: **HPF Cutoff** (20 Hz – 18 kHz, exp taper). Set it just above the fundamental of your lowest played note to remove rumble without thinning the body.

The HPF is a 1-pole (6 dB/oct) topology-preserving design with no resonance. It sits *before* the main filter, so the ladder receives an already-trimmed signal.

## Main filter (VCF)

**Cutoff** (16.35 Hz – 16 kHz, exp taper centred at 800 Hz) controls the corner frequency. The taper sits a little above middle of the knob travel by default — this matches typical analog control voltages and gives even resolution around the most musically useful region.

**Resonance** (0–1, linear) increases feedback around the cutoff. The ladder will self-oscillate cleanly at the top of the range; settings around 0.5–0.7 give the characteristic emphasised cutoff peak without howl.

**Drive** (0.1–4, exp taper, default 1.0) saturates the input to the ladder. Below 1 the filter behaves cleaner / softer; above 1 the input clips into the per-stage `tanh` saturators (rational Padé(5,6) approximation, applied at each integrator input) and harmonic content blooms around the cutoff. Useful for adding bite without raising master volume.

**Filter Mode** selects which point on the ladder is tapped:

| Mode | Behaviour |
| --- | --- |
| **LP** | Classic ladder low-pass (default). |
| **HP** | High-pass output from the ladder (in addition to the pre-VCF HPF). |
| **BP** | Bandpass centred at cutoff, Q proportional to resonance. |
| **Notch** | Band-reject (inverse of bandpass). |

**Filter Slope** picks 12 dB/oct (2-pole) or 24 dB/oct (4-pole). 24 dB is the default — fatter, more "ladder-like." 12 dB is brighter and lets more upper harmonics through, useful for leads where you want presence even at low cutoff.

## Key Track

**Key Track** (0–1) is a continuous depth knob. At 1.0 the cutoff rises one octave per octave of key relative to C4 (the played note's pitch tracks 1:1 into the filter). At 0 the cutoff stays fixed regardless of key. Intermediate values give proportional tracking — 0.5 is half-tracking, etc.

Set this to 1.0 for a filter that follows the keyboard fully; use lower values when you want bass notes to be darker than treble notes without going all the way to neutral.

## Modulation

Filter modulation has **four fixed depths** with no source selector — every depth is live simultaneously. See [Filter modulation](modulation.md#filter-modulation):

- **Cutoff LFO1 Dep** — LFO 1 (per-voice) into cutoff.
- **Cutoff LFO2 Dep** — LFO 2 (global) into cutoff.
- **Cutoff Env Dep** — Env 1 into cutoff. Can be negative (envelope closes the filter).
- **Vel→Cutoff** — MIDI velocity into cutoff.

Plus from the [Mod Wheel panel](modulation.md#mod-wheel-routes):

- **Wheel→Cutoff** — MIDI CC1 into cutoff.
- **Wheel→Reso** — CC1 into resonance.

## Parameters

| Parameter | Range | Default | Unit | Notes |
| --- | --- | --- | --- | --- |
| HPF Cutoff | 20–18000 | 20 | Hz | Pre-VCF high-pass |
| Cutoff | 16.35–16000 | 1000 | Hz | Exp taper (mid 800 Hz) |
| Resonance | 0–1 | 0.2 | linear | |
| Drive | 0.1–4 | 1.0 | linear | Exp taper (mid 1.0) |
| Filter Mode | LP / HP / BP / Notch | LP | enum | Ladder output selector |
| Filter Slope | 12 dB / 24 dB | 24 dB | enum | 2-pole or 4-pole |
| Key Track | 0–1 | 0 | linear | 1.0 = 1 octave cutoff / octave key |
| Tuned | Off / On | Off | bool | Reserved for future cutoff-tuned-to-key offset behaviour; currently inactive |
