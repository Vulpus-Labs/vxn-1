# Parameter reference

The complete VXN1 parameter table, grouped by panel. **156 parameters total** = `2 × 64` per-layer + `28` global (per ADR 0001).

Per-layer parameters are exposed twice to the host — once for Upper, once for Lower — and described once below.

## Oscillator 1

| Parameter | Range | Default | Unit |
| --- | --- | --- | --- |
| Osc 1 Wave | Sine / Tri / Saw / Pulse | Saw | enum |
| Osc 1 Octave | −4 to +4 | 0 | oct |
| Osc 1 Coarse | −7 to +7 | 0 | st |
| Osc 1 Fine | −50 to +50 | 0 | ct |
| Osc 1 Level | 0–1 | 0.8 | linear |
| Osc 1 PW | 0.05–0.95 | 0.5 | duty |

## Oscillator 2 & Cross-Mod

| Parameter | Range | Default | Unit |
| --- | --- | --- | --- |
| Osc 2 Wave | Sine / Tri / Saw / Pulse | Saw | enum |
| Osc 2 Octave | −4 to +4 | −1 | oct |
| Osc 2 Coarse | −7 to +7 | 0 | st |
| Osc 2 Fine | −50 to +50 | 0 | ct |
| Osc 2 Level | 0–1 | 0.6 | linear |
| Osc 2 PW | 0.05–0.95 | 0.5 | duty |
| Cross-Mod Type | Off / Sync / FM / Ring | Off | enum |
| Cross-Mod Amount | 0–4 | 0 | linear |

## Mixer

| Parameter | Range | Default | Unit |
| --- | --- | --- | --- |
| Sub Level | 0–1 | 0 | linear |
| Noise Level | 0–1 | 0 | linear |
| Noise Colour | White / Pink | White | enum |

## Filter

| Parameter | Range | Default | Unit |
| --- | --- | --- | --- |
| HPF Cutoff | 20–18000 | 20 | Hz |
| Cutoff | 16.35–16000 | 1000 | Hz |
| Resonance | 0–1 | 0.2 | linear |
| Drive | 0.1–4 | 1.0 | linear |
| Filter Mode | LP / HP / BP / Notch | LP | enum |
| Filter Slope | 12 dB / 24 dB | 24 dB | enum |
| Key Track | Off / On | Off | bool |

## Envelope 1 (Modulation)

| Parameter | Range | Default | Unit |
| --- | --- | --- | --- |
| Env 1 Attack | 0.001–10 | 0.005 | s |
| Env 1 Decay | 0.001–10 | 0.3 | s |
| Env 1 Sustain | 0–1 | 0 | linear |
| Env 1 Release | 0.001–10 | 0.3 | s |
| Env 1 Shape | Linear / Exp | Linear | enum |

## Envelope 2 (Amplitude) + VCA

| Parameter | Range | Default | Unit |
| --- | --- | --- | --- |
| Env 2 Attack | 0.001–10 | 0.005 | s |
| Env 2 Decay | 0.001–10 | 0.2 | s |
| Env 2 Sustain | 0–1 | 0.8 | linear |
| Env 2 Release | 0.001–10 | 0.3 | s |
| Env 2 Shape | Linear / Exp | Exp | enum |
| Amp Gate | Off / On | Off | bool |
| Amp LFO | Off / LFO 1 / LFO 2 | Off | enum |
| Amp LFO Dep | 0–1 | 0 | linear |

## LFO 1 (per-voice, per-layer)

| Parameter | Range | Default | Unit |
| --- | --- | --- | --- |
| LFO 1 Shape | Sine / Tri / Saw+ / Saw− / Sq / S&H | Sine | enum |
| LFO 1 Rate | 0.01–40 | 5.0 | Hz |
| LFO 1 Sync | Off / On | Off | bool |
| LFO 1 Delay | 0–4 | 0 | s |
| LFO 1 Fade | 0–4 | 0 | s |
| LFO 1 Free | Off / On | Off | bool |

## Pitch modulation

| Parameter | Range | Default | Unit |
| --- | --- | --- | --- |
| Pitch LFO | Off / LFO 1 / LFO 2 | LFO 1 | enum |
| Pitch LFO Dep | 0–12 | 0.05 | st |
| Pitch LFO Mod | Off / On | Off | bool |
| Pitch Env | Off / Env 1 / Env 2 | Off | enum |
| Pitch Env Dep | −12 to +12 | 0 | st |
| Pitch Env Mod | Off / On | Off | bool |
| Pitch Wheel | 0–12 | 2.0 | st |

## PWM modulation

| Parameter | Range | Default | Unit |
| --- | --- | --- | --- |
| PWM LFO | Off / LFO 1 / LFO 2 | Off | enum |
| PWM LFO Dep | 0–0.5 | 0 | linear |
| PWM Env | Off / Env 1 / Env 2 | Off | enum |
| PWM Env Dep | −0.5 to +0.5 | 0 | linear |

## Filter modulation

| Parameter | Range | Default | Unit |
| --- | --- | --- | --- |
| Cutoff LFO1 Dep | 0–48 | 0 | st |
| Cutoff LFO2 Dep | 0–48 | 0 | st |
| Cutoff Env Dep | −96 to +96 | 0 | st |
| Vel→Cutoff | −96 to +96 | 0 | st |

## Cross-Mod Sweep

| Parameter | Range | Default | Unit |
| --- | --- | --- | --- |
| X-Mod Sweep Env | Off / Env 1 / Env 2 | Off | enum |
| X-Mod Sweep Env Dep | −48 to +48 | 0 | st |

## Mod Wheel routes

| Parameter | Range | Default | Unit |
| --- | --- | --- | --- |
| Wheel→PWM | −0.5 to +0.5 | 0 | linear |
| Wheel→Cutoff | −96 to +96 | 0 | st |
| Wheel→Reso | 0–1 | 0 | linear |
| Wheel→X-Mod Sweep | −48 to +48 | 0 | st |

## Voice & assign

| Parameter | Range | Default | Unit |
| --- | --- | --- | --- |
| Assign | Poly / Unison / Solo / Twin | Poly | enum |
| Legato | Off / On | Off | bool |
| Unison Detune | 0–50 | 12 | ct |
| Glide Time | 0–0.5 | 0 | s |
| Layer Level | 0–1 | 1.0 | linear |
| Spread | 0–1 | 0 | linear |

## Global — Master

| Parameter | Range | Default | Unit |
| --- | --- | --- | --- |
| Master Tune | −12 to +12 | 0 | st |
| Master Volume | 0–1 | 0.7 | linear |
| Master Drift | 0–1 | 0 | linear |
| Limiter | Off / On | Off | bool |
| Oversample | Off / 2× / 4× / 8× | 2× | enum |

## Global — LFO 2

| Parameter | Range | Default | Unit |
| --- | --- | --- | --- |
| LFO 2 Shape | Sine / Tri / Saw+ / Saw− / Sq / S&H | Sine | enum |
| LFO 2 Rate | 0.01–40 | 5.0 | Hz |
| LFO 2 Sync | Off / On | Off | bool |

## Global — Phaser

| Parameter | Range | Default | Unit |
| --- | --- | --- | --- |
| Phaser | Off / On | Off | bool |
| Phaser Rate | 0.05–10 | 0.5 | Hz |
| Phaser Depth | 0–1 | 0.7 | linear |
| Phaser FB | −0.9 to +0.9 | 0 | linear |
| Phaser Mix | 0–1 | 0.5 | linear |

## Global — Chorus

| Parameter | Range | Default | Unit |
| --- | --- | --- | --- |
| Chorus | Off / On | On | bool |
| Chorus Rate | 0.05–8 | 0.6 | Hz |
| Chorus Depth | 0–1 | 0.5 | linear |
| Chorus Mix | 0–1 | 0.4 | linear |

## Global — Delay

| Parameter | Range | Default | Unit |
| --- | --- | --- | --- |
| Delay | Off / On | Off | bool |
| Delay Time | 0.01–2 | 0.35 | s |
| Delay FB | 0–0.95 | 0.4 | linear |
| Delay Mix | 0–1 | 0.25 | linear |
| Delay Sync | Off / On | Off | bool |

## Global — Reverb

| Parameter | Range | Default | Unit |
| --- | --- | --- | --- |
| Reverb | Off / On | Off | bool |
| Reverb Size | 0–1 | 0.5 | linear |
| Reverb Decay | 0.2–10 | 2.5 | s |
| Reverb Damp | 0–1 | 0.4 | linear |
| Reverb Mix | 0–1 | 0.3 | linear |

## Non-parameter state

These live in plugin state (`PluginState`) but are not host-automatable:

- **Key Mode** — Whole / Dual / Split
- **Split Point** — MIDI note (default 60)
- **Layer Switcher** — UI selection (Upper / Lower); persists across project reopens
