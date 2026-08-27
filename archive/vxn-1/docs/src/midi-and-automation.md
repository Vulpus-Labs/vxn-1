# MIDI & automation

VXN1 takes standard MIDI and CLAP / VST3 parameter automation. This page covers what each input type drives.

## MIDI notes

- **Note On / Note Off** — drive the voice allocator according to the layer's [Assign Mode](panels/voice.md#assign-mode) and the instrument's [Key Mode](key-modes.md).
- **Velocity** — routed by the **Vel→Cutoff** depth on the filter modulation panel. Velocity is *not* hardwired to amplitude; if you want velocity-to-volume, route an envelope through it.

## MIDI controllers

| MIDI input | Default routing | Destination panel |
| --- | --- | --- |
| **Pitch Wheel** | ±2 st on both osc | [Pitch modulation](panels/modulation.md#pitch-modulation) — `Pitch Wheel` knob (0–12 st) |
| **Mod Wheel (CC1)** | All four routes default 0 | [Mod Wheel routes](panels/modulation.md#mod-wheel-routes) — Wheel→PWM, Wheel→Cutoff, Wheel→Reso, Wheel→X-Mod Sweep |
| **Channel Aftertouch / Poly Aftertouch** | Not routed | Reserved for future revisions |
| **Sustain (CC64)** | Standard sustain (holds gate after note-off) | All assign modes |

The Mod Wheel is smoothed with a 40 ms time constant at control rate to filter controller jitter.

## Tempo sync

Three parameters honour host tempo when their **Sync** toggle is on:

- **LFO 1 Sync**
- **LFO 2 Sync**
- **Delay Sync**

With sync on, the rate knob steps through beat subdivisions (1/1 down to 1/32, including triplet and dotted variants). With sync off, the rate is free-running in Hz.

## Parameter automation

All 165 parameters are exposed to CLAP automation (and to VST3 once the wrapper build lands; see [Distribution](internals/distribution.md)). The full list lives in the [Parameter reference](parameter-reference.md). A few notes on automation behaviour:

- **Per-layer parameters** are exposed twice — once for Upper, once for Lower. Their CLAP IDs are derived from a per-layer offset.
- **Key Mode** and **Split Point** are *not* automatable. They live in plugin state and can be saved / loaded with the project but can't be moved by an automation lane (ADR 0003).
- **Performance** parameters (mod wheel routings, pitch wheel range) are automatable but tend to be set-and-forget — they're per-layer state that you'd normally save in a preset.
- **Smoothing**: every continuous parameter is smoothed at control rate (typically 32-sample blocks). Step-change automation won't produce zipper artefacts.

### VST3 IDs

VST3 parameter IDs are derived from CLAP IDs by hashing. This means **renaming a CLAP parameter ID will break VST3 automation in saved projects**. Identifier stability is a soft policy from ADR 0008 — VXN1 won't rename IDs post-ship except in release notes. If you write DAW automation against VST3, the IDs are stable across patch versions.

## Patch reload after preset change

When a preset loads via the browser, the plugin posts the new parameter values to the host so automation lanes refresh. This works in CLAP. In VST3, host behaviour varies — some hosts re-read parameter values on a *plugin* request, others only on user interaction. Test in your specific host.
