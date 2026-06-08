# Effects

Four global effects sit between the voice mix and the master stage, in this order:

```
voice mix ─► Phaser ─► Chorus ─► Delay ─► Reverb ─► master
```

All four are **global** (shared by both layers) and each can be turned on or off independently.

## Phaser

Pre-chorus phaser. Four cascaded all-pass stages with an LFO sweeping the centre frequency.

| Parameter | Range | Default | Unit | Notes |
| --- | --- | --- | --- | --- |
| Phaser | Off / On | Off | bool | |
| Phaser Rate | 0.05–10 | 0.5 | Hz | LFO rate, exp taper (mid 1 Hz) |
| Phaser Depth | 0–1 | 0.7 | linear | Sweep range |
| Phaser FB | −0.9 to +0.9 | 0 | linear | Feedback (negative inverts) |
| Phaser Mix | 0–1 | 0.5 | linear | Dry/wet |

## Chorus

Vintage BBD (bucket-brigade) chorus, "Bright" voicing modelled on the Juno-60. Includes bucket-write saturation, reconstruction filter at 9 kHz, and post-BBD makeup gain.

The right channel reads the *inverted* LFO phase — authentic mono-compatible stereo, not two offset LFOs.

| Parameter | Range | Default | Unit | Notes |
| --- | --- | --- | --- | --- |
| Chorus | Off / On | On | bool | Default on |
| Chorus Rate | 0.05–8 | 0.6 | Hz | BBD LFO rate |
| Chorus Depth | 0–1 | 0.5 | linear | Delay swing amount |
| Chorus Mix | 0–1 | 0.4 | linear | Dry/wet |

Delay range: 1.66–5.35 ms, swept by a strict-triangle LFO.

## Delay

Stereo delay with feedback.

| Parameter | Range | Default | Unit | Notes |
| --- | --- | --- | --- | --- |
| Delay | Off / On | Off | bool | |
| Delay Time | 0.01–2 | 0.35 | s | Linear taper |
| Delay FB | 0–0.95 | 0.4 | linear | Feedback amount |
| Delay Mix | 0–1 | 0.25 | linear | Dry/wet |
| Delay Sync | Off / On | Off | bool | Host-tempo sync (planned; not yet routed) |

## Reverb

Feedback delay network (FDN) reverb. Size controls the FDN's perceived room dimensions; damp absorbs high frequencies on each feedback pass.

| Parameter | Range | Default | Unit | Notes |
| --- | --- | --- | --- | --- |
| Reverb | Off / On | Off | bool | |
| Reverb Size | 0–1 | 0.5 | linear | Room size |
| Reverb Decay | 0.2–10 | 2.5 | s | Decay time, exp taper (mid 2.0) |
| Reverb Damp | 0–1 | 0.4 | linear | High-freq damping |
| Reverb Mix | 0–1 | 0.3 | linear | Dry/wet |

## Bypass behaviour

Each effect's on/off toggle hard-bypasses that stage. Bypassed stages have no CPU cost — the audio buffer passes through unaltered. There is no smoothing on the bypass toggle, so automating it mid-note will click — use the **Mix** knob for click-free wet/dry changes.
