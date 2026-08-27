# Mixer

Five sources feed the filter:

- **Osc 1** at its **Level** knob (mixer column on the Osc 1 panel).
- **Osc 2** at its **Level** knob (mixer column on the Osc 2 panel).
- **Sub** — a square wave one octave below Osc 1, useful for adding low-end weight without retuning the main oscillators.
- **Noise** — white or pink, selected by the **Noise Colour** toggle.

All four sources sum into a single stereo mix before the high-pass and ladder filter.

## Sub-oscillator

The sub is a square wave one octave below the carrier oscillator. By default that's Osc 1; under **Cross-Mod Type = Sync** the sub follows Osc 2 instead (since Osc 1 is then a sync slave of Osc 2). Otherwise the sub tracks Osc 1's tuning, including pitch modulation, and is band-limited.

A common move: pull Osc 1 Level to ~0.5, raise Sub Level to ~0.7 for a fat bass that retains its top-octave brightness from Osc 2.

## Noise

**Noise Colour**:

- **White** — uniform spectral density.
- **Pink** — −3 dB/oct rolloff (1/f). Friendlier on the ladder filter at high resonance.

A small amount of noise (level ~0.1) blended with the oscillators gives a perceptual "lift" — the analog impression of subtle hiss in the signal path. Larger amounts move into wind and percussion territory; use the modulation envelope to gate noise bursts for snare-like attacks.

## Parameters

| Parameter | Range | Default | Unit | Notes |
| --- | --- | --- | --- | --- |
| Sub Level | 0–1 | 0 | linear | Square one octave below Osc 1 |
| Noise Level | 0–1 | 0 | linear | |
| Noise Colour | White / Pink | White | enum | |

Osc 1 Level and Osc 2 Level are documented on the [Oscillators](oscillators.md) page.
