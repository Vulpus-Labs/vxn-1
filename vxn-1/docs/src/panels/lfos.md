# LFOs

VXN1 carries two LFOs with deliberately different scopes:

- **LFO 1** is **per-voice**. Each voice owns a phase; default behaviour retriggers on note-on. Has delay / fade-in for ease-in vibrato.
- **LFO 2** is **global**. One phase shared across every voice in both layers. Useful when you want the whole instrument moving together — wobble basses, dual-osc detune drift, synchronised filter cycling.

## Waveforms

Both LFOs share the same six shapes:

| Value | Shape |
| --- | --- |
| 0 | Sine |
| 1 | Triangle |
| 2 | Saw+ (rising) |
| 3 | Saw− (falling) |
| 4 | Square |
| 5 | Sample & Hold |

## Rate and sync

**Rate** (0.01–40 Hz, exponential taper centred at 5 Hz) is free-running by default.

**Sync** locks the LFO rate to host tempo. With sync on, the rate knob steps through beat subdivisions instead of Hertz (1/1, 1/2, 1/4, 1/8, 1/16, 1/32, 1/8T, 1/16T, …). The control-rate update cadence is unchanged.

## LFO 1 specifics

**Free-Run** (off by default) controls phase retriggering:

- **Off** — phase resets to 0 on every note-on. Predictable vibrato that always starts from zero.
- **On** — phase runs continuously across notes. Better for chord-spanning LFO sweeps where you don't want every voice's phase coupled to its note-on time.

**Delay Time** (0–4 s) sets a hold before the LFO becomes audible after note-on. Useful for slow-onset vibrato that only kicks in once the note has been held.

**Fade** (0–4 s) is the ramp-up time once the delay expires — the LFO's output amplitude crossfades from 0 to its target over this interval. Combined with Delay Time, this gives the classic "vibrato that grows" voice mannerism.

LFO 1 has no delay / fade for cases where Free-Run is on — the LFO is continuous, so delay / fade has no anchor point.

## LFO 2 specifics

LFO 2 has just Shape / Rate / Sync. There's no delay or fade because the global LFO has no per-note event to trigger from. (It also has no Free-Run option because it's *always* free-running — that's what global means.)

## Where the LFOs go

Both LFOs are sources on every modulation panel:

- [Pitch](modulation.md#pitch-modulation) — vibrato (±12 st).
- [PWM](modulation.md#pwm-modulation) — pulse-width wobble.
- [Filter Cutoff](modulation.md#filter-modulation) — filter sweeps. (LFO 1 and LFO 2 each have their own fixed depth knob.)
- [Amp](envelopes.md#tremolo) — tremolo.

## Parameters — LFO 1 (per-layer)

| Parameter | Range | Default | Unit | Notes |
| --- | --- | --- | --- | --- |
| LFO 1 Shape | Sine / Tri / Saw+ / Saw− / Sq / S&H | Sine | enum | |
| LFO 1 Rate | 0.01–40 | 5.0 | Hz | Exp taper |
| LFO 1 Sync | Off / On | Off | bool | Host-tempo sync |
| LFO 1 Delay | 0–4 | 0 | s | Pre-fade hold |
| LFO 1 Fade | 0–4 | 0 | s | Fade-in ramp |
| LFO 1 Free | Off / On | Off | bool | 1 = free-running, 0 = retrigger on note-on |

## Parameters — LFO 2 (global)

| Parameter | Range | Default | Unit | Notes |
| --- | --- | --- | --- | --- |
| LFO 2 Shape | Sine / Tri / Saw+ / Saw− / Sq / S&H | Sine | enum | |
| LFO 2 Rate | 0.01–40 | 5.0 | Hz | Exp taper |
| LFO 2 Sync | Off / On | Off | bool | |
