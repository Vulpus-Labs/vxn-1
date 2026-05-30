---
id: "0042"
title: HTML faceplate — LFO 1 (per-voice) + LFO 2 (global) panels
priority: high
created: 2026-05-30
epic: E010
---

## Summary

Implement Row 1's LFO panels. LFO 1 is per-voice (Shape, Rate, Sync,
Delay, Fade, Free); LFO 2 is one global oscillator (Shape, Rate,
Sync). Host-sync displays the rate fader as a musical subdivision
label instead of Hz when Sync is on.

## Acceptance criteria

- [ ] LFO 1 panel: Shape (rotary), Rate (fader), Sync (switch), Delay
      (fader), Fade (fader), Free (switch). Per-layer (layer-aware
      placeholder per 0041).
- [ ] LFO 2 panel: Shape (rotary), Rate (fader), Sync (switch).
      Global (not layer-aware).
- [ ] When Sync is on, the Rate fader's display reads as a musical
      subdivision (`"1/4"`, `"1/8T"`, etc.) — taken from the
      descriptor `display` string in the `ParamChanged` ViewEvent.
      The fader's normalised value still drives the engine; only the
      readout label changes.
- [ ] LFO 1's per-voice quirks (the Free toggle disables delay/fade
      to keep the LFO free-running) — visual feedback only (dimmed
      controls), no client-side state machine.
- [ ] DAW automation playback exercises both LFOs visibly.

## Notes

Subdivision display comes from the descriptor; the editor never
formats labels itself (ADR 0001: "host and UI render identically").
Same rule as Vizia.

The "Free" switch dim-on-active visual was a quirk added to the
Vizia editor late; keep it but treat as decoration — the engine
behaviour is in the param descriptor.
