# Oscillators

Each layer has two full-feature oscillators. They share the same parameter set; defaults differ (Osc 2 is one octave below Osc 1) so the Init patch sounds layered rather than unisoned.

## Waveforms

Both oscillators offer four waveforms:

| Value | Waveform | Character |
| --- | --- | --- |
| 0 | **Sine** | Pure sine, low harmonics. Useful as a PM modulator or sub-fundamental. |
| 1 | **Triangle** | Soft, mostly odd harmonics. Reedier than sine but smoother than saw. |
| 2 | **Saw** | Full harmonic content, bright and dc-free. Default. |
| 3 | **Pulse** | Square at PW 0.5; narrows toward hollow / nasal at PW 0.05 or 0.95. Duty cycle set by **PW** parameter or PWM modulation. |

The Saw and Pulse waveforms are band-limited via polyBLEP residuals — no zipper aliasing on sweeps, even at extreme oversampling settings.

## Tuning

Three knobs stack:

- **Octave** (−4 to +4 oct) — coarse octave transposition.
- **Coarse** (−7 to +7 st) — semitone fine-tune, useful for 5ths / 7ths above Osc 1.
- **Fine** (−50 to +50 ct) — cent detune, used most often on Osc 2 to thicken the pair.

Per-voice **Drift** (Master panel) adds a small random per-voice phase offset to both oscillators, modelling analogue tuning instability.

## Pulse Width

Static **PW** (0.05–0.95) sets the square's duty cycle. At 0.5 it's a perfect square; values toward 0.05 / 0.95 give a thinner, more nasal tone.

For *moving* pulse width, see the [PWM modulation route](modulation.md#pwm-modulation) — LFO or envelope sources can sweep the width on top of the static setting.

## Parameters — Osc 1

| Parameter | Range | Default | Unit | Notes |
| --- | --- | --- | --- | --- |
| Osc 1 Wave | Sine / Tri / Saw / Pulse | Saw | enum | |
| Osc 1 Octave | −4 to +4 | 0 | oct | |
| Osc 1 Coarse | −7 to +7 | 0 | st | |
| Osc 1 Fine | −50 to +50 | 0 | ct | |
| Osc 1 Level | 0–1 | 0.8 | linear | Mixer level |
| Osc 1 PW | 0.05–0.95 | 0.5 | duty | Pulse waveform only |

## Parameters — Osc 2

| Parameter | Range | Default | Unit | Notes |
| --- | --- | --- | --- | --- |
| Osc 2 Wave | Sine / Tri / Saw / Pulse | Saw | enum | |
| Osc 2 Octave | −4 to +4 | −1 | oct | Defaults one octave below Osc 1 |
| Osc 2 Coarse | −7 to +7 | 0 | st | |
| Osc 2 Fine | −50 to +50 | 0 | ct | |
| Osc 2 Level | 0–1 | 0.6 | linear | |
| Osc 2 PW | 0.05–0.95 | 0.5 | duty | |

In **Sync** mode, Osc 1 becomes the sync slave and Osc 2 the master — see [Cross-modulation](cross-modulation.md).
