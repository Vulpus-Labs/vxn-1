# Quick start

This page walks you from a fresh VXN1 instance to a playable sound in about a minute.

## Load the plugin

In your DAW, create a MIDI track and insert **VXN1** as the instrument. The default patch is a saw-saw poly sound with chorus on — play a chord and you should hear a clear, slightly-detuned pad.

## A tour of the faceplate

The default patch loads the **Init** state, which is a deliberately neutral starting point:

| Section | Default |
| --- | --- |
| Osc 1 | Saw, octave 0, level 0.8 |
| Osc 2 | Saw, octave −1, level 0.6 |
| Sub / Noise | Off |
| Filter | LP, 24 dB/oct, cutoff 1 kHz, resonance 0.2, drive 1.0 |
| Env 1 (mod) | A 5 ms / D 300 ms / S 0 / R 300 ms, linear |
| Env 2 (amp) | A 5 ms / D 200 ms / S 0.8 / R 300 ms, exponential |
| LFO 1 | Sine, 5 Hz, free-running off |
| LFO 2 | Sine, 5 Hz |
| Chorus | On, rate 0.6 Hz, mix 0.4 |
| Oversample | 2× |

Both oscillators are at full saw, and Env 2 (the amplitude envelope) is hardwired to the VCA — you'll always hear sound at note-on.

## Make a bass

1. **Filter**: drop cutoff to ~400 Hz, push resonance to 0.5, drive to 1.5.
2. **Env 1**: sustain 0, decay 200 ms.
3. **Filter Mod**: set `Cutoff Env Dep` to about +48 (one octave open with each note).
4. **Voice**: change Assign to **Solo**, set Glide Time to 30–50 ms.
5. **Chorus**: turn off for a drier bass tone.

## Make a pad

1. **Env 2**: attack 800 ms, release 1.5 s, sustain 0.8.
2. **Filter**: cutoff 2 kHz, slope 24 dB, resonance 0.1.
3. **Filter Mod**: `Cutoff LFO2 Dep` ≈ +12, **LFO 2 Rate** ≈ 0.3 Hz for slow filter sweeps.
4. **Pitch Mod**: LFO source = LFO 1, depth ≈ 0.1 st for subtle vibrato.
5. **Chorus**: depth 0.7, mix 0.5.
6. **Reverb**: on, size 0.6, decay 4 s, mix 0.35.

## Make a lead

1. **Voice**: Assign = **Solo**, Legato = on, Glide Time 80 ms.
2. **Cross-Mod Type** = **Sync**, **Cross-Mod Amount** ~1.5, Osc 2 Coarse +7 — classic sync-lead character.
3. **Env 1**: short attack, decay 150 ms, sustain 0; `Cutoff Env Dep` +24 for the snap.
4. **Mod Wheel**: route Wheel→PWM 0.2, Wheel→Cutoff +18 — your wheel becomes a brightness/movement control.

## Save your patch

The **preset bar** at the top of the faceplate (between the banner and the first row of controls) handles save/load: `< name >` walks through the combined factory + user list, **Browse** opens the folder/preset panel, **Save** overwrites the current user preset, **Save As** opens a name/folder dialog. New user presets land in:

- **macOS**: `~/Library/Audio/Presets/Vulpus Labs/VXN1/`
- **Windows**: `%APPDATA%\Vulpus Labs\VXN1\Presets`
- **Linux**: `$XDG_DATA_HOME/VXN1/presets` (fallback: `~/.local/share/VXN1/presets`)

See [Presets](presets.md) for the full preset model.

## Next steps

- [Mental model](mental-model.md) — how VXN1's signal path and modulation work conceptually.
- [Faceplate reference](panels/overview.md) — every knob explained, panel by panel.
- [Key modes](key-modes.md) — split, dual, and layered playing.
