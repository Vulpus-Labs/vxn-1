# Voice & assign

This panel controls how MIDI notes map to voices within a single layer. Layer-level decisions about *which* layer receives MIDI live on the [Key Modes page](../key-modes.md).

Each layer has **8 channels**. Assign Mode picks how those channels are spent.

## Assign Mode

| Mode | Channels per note | Polyphony | Notes |
| --- | --- | --- | --- |
| **Poly** | 1 | 8 (per layer) | First-free voice, oldest-steal when full. Standard polyphonic behaviour. |
| **Unison** | 8 | 1 | All 8 channels stack on every note. Per-channel detune (`Unison Detune`) and phase decorrelation. Mono — one note at a time. Level-compensated by 1/√8. |
| **Solo** | 1 | 1 | One channel, last-note priority. With Legato on, the envelope doesn't retrigger when you slur. |
| **Twin** | 2 | 4 | Two channels per note at ±`Unison Detune` and a 90° phase offset; 1/√2 level compensation. Effectively halves polyphony to thicken each note. |

In **Whole** key mode, **Poly** spreads notes across both layers (16-voice round-robin) and **Twin** doubles to 8-note. **Unison** and **Solo** are inherently mono and run on the Upper layer only — the second layer's channels are idle. In **Dual** and **Split**, each layer is independent: you can have Upper in Solo for the melody and Lower in Poly for the chord, for instance.

## Glide (Portamento)

**Glide Time** (0–0.5 s, exp taper) sets the per-voice pitch slide time. It applies in *all* assign modes — Solo, Twin, Unison, even Poly (where each new voice glides from where its channel last left off).

**Legato** only changes behaviour in **Solo** mode:

- **Legato Off** — every new note retriggers the gate (and so retriggers Env 1 / Env 2 and LFO 1 unless Free-Run is on).
- **Legato On** — overlapping new notes glide but don't retrigger. Use this for expressive lead lines where you want continuous envelope decay through a phrase.

In Poly mode, Legato is silently ignored (every new note takes a fresh channel; there's nothing to "slur into").

## Unison Detune

**Unison Detune** (0–50 ct) is the per-channel detune spread for **Unison** and **Twin** modes. In Unison, all 8 channels are spread across the ±Detune range. In Twin, the two channels sit at +Detune and −Detune exactly.

A small value (5–10 ct) gives subtle thickening; larger values (25–40 ct) move into chorus / ensemble territory.

## Layer level and Spread

These two parameters live on the same panel but are independent of assign mode:

- **Layer Level** (0–1, default 1.0) — per-layer gain applied after rendering, before effects. Used to balance Upper and Lower in Dual mode.
- **Spread** (0–1, default 0) — pans voice slots across the stereo field. At 0, all voices are centred. At 1, voices fan out into a wide stereo image. The spread is per-voice-slot, not per-note, so a single note hits the same pan position across plays.

## Parameters

| Parameter | Range | Default | Unit | Notes |
| --- | --- | --- | --- | --- |
| Assign | Poly / Unison / Solo / Twin | Poly | enum | |
| Legato | Off / On | Off | bool | Solo mode only |
| Unison Detune | 0–50 | 12 | ct | Per-channel detune spread |
| Glide Time | 0–0.5 | 0 | s | Exp taper (mid 0.1) |
| Layer Level | 0–1 | 1.0 | linear | Per-layer gain |
| Spread | 0–1 | 0 | linear | Stereo voice spread |
