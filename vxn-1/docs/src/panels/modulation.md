# Modulation routes

Modulation in VXN1 is organised by **destination**: each routable destination is its own panel with source selectors and depth knobs. There is no matrix. Every route on this page is always live — no routing menus, no slots to assign.

## Pitch modulation

Routes to both oscillators by default (vibrato, pitch envelope, pitch wheel). The two **Mod** switches turn either route into a single-oscillator route to drive the cross-mod modulator without moving the carrier — which is how VXN1 implements wide cross-mod sweeps.

- **Pitch LFO** — source: Off / LFO 1 / LFO 2.
- **Pitch LFO Dep** (0–12 st, exp taper) — vibrato depth.
- **Pitch LFO Mod** — when on, the LFO contribution is routed only to the cross-mod *modulator* oscillator. Under **Sync**, that's Osc 1 (the slave). Under **PM / Ring / Off**, that's Osc 2 (whose output modulates Osc 1). When off, the LFO route moves both oscillators together (normal vibrato).
- **Pitch Env** — source: Off / Env 1 / Env 2.
- **Pitch Env Dep** (−12 to +12 st) — envelope-driven pitch sweep. Negative inverts.
- **Pitch Env Mod** — same single-oscillator routing as the LFO Mod switch. With Mod on and a wide envelope depth (use the ±12 st range), this is the standard way to drive a classic sync sweep or a swept PM index.
- **Pitch Wheel** (0–12 st) — pitch-bend range from MIDI pitch wheel. Always applies to both oscillators.

The Mod switches and the **Cross-Mod Sweep** mod-wheel route below are the only routes that target one oscillator without the other. All other pitch routing moves both osc together.

## PWM modulation

Routes to **Osc 1 and Osc 2 pulse widths** simultaneously (so PWM only affects oscillators currently set to Pulse).

- **PWM LFO** — source: Off / LFO 1 / LFO 2.
- **PWM LFO Dep** (0 to 0.5) — sweep depth (0.25 = full ±25% PW excursion).
- **PWM Env** — source: Off / Env 1 / Env 2.
- **PWM Env Dep** (−0.5 to +0.5) — envelope depth, can invert.

Static PW from each oscillator's PW knob is the centre point; modulation displaces from there.

## Filter modulation

Four fixed depths, **no source selectors**. Every depth knob is live simultaneously; set unused routes to zero. The envelope route is hardwired to Env 1.

- **Cutoff LFO1 Dep** (0–48 st) — LFO 1 → cutoff.
- **Cutoff LFO2 Dep** (0–48 st) — LFO 2 → cutoff.
- **Cutoff Env Dep** (−96 to +96 st) — Env 1 → cutoff (source is fixed; not selectable). Negative *closes* the filter on note-on.
- **Vel→Cutoff** (−96 to +96 st) — MIDI velocity → cutoff.

The 96-semitone range gives enough headroom to fully sweep from minimum to maximum cutoff via velocity alone — useful for dynamic playing where soft notes are dark and hard notes are bright.

Key tracking is a separate continuous depth on the [Filter panel](filter.md#key-track), not a modulation depth on this panel.

## Cross-Mod Sweep (mod-wheel)

VXN1 has no dedicated cross-mod sweep envelope route. Instead:

- **Envelope-driven sweeps** are built by enabling the **Mod** switch on the Pitch Env route (above). With Mod on, the pitch envelope drives only the cross-mod modulator (Osc 1 in Sync mode; Osc 2 in PM / Ring / Off). Use the full ±12 st depth for dramatic sweeps; combine with Osc 2 detune to shift the sweep range.
- **Wheel-driven sweeps** are the **Wheel→X-Mod** knob in the Mod Wheel panel below, which gives ±48 st of wide pitch range. Unlike the Mod-switched routes, the wheel route is *always* both-osc — under Sync and Ring it shifts both oscillators in parallel; under PM the modulator's pitch dominates the audible result.

The wheel route is *not* gated by Cross-Mod Type — turning Cross-Mod Type to Off doesn't disable it. With Off or Ring it acts as a wide pitch joystick.

## Mod Wheel routes

The Mod Wheel (MIDI CC1) has four fixed destinations. Set any to zero to disable that route.

- **Wheel→PWM** (−0.5 to +0.5) — pulse-width depth.
- **Wheel→Cutoff** (−96 to +96 st) — cutoff depth.
- **Wheel→Reso** (0–1) — adds to resonance.
- **Wheel→X-Mod** (−48 to +48 st) — wide pitch route; used as the cross-mod sweep when Cross-Mod Type is Sync or PM.

The Mod Wheel CC is smoothed with a 40 ms time constant at control rate, so jitter from cheap controllers won't transmit into the audio.

## Parameter summary

| Parameter | Range | Default | Unit |
| --- | --- | --- | --- |
| Pitch LFO | Off / LFO 1 / LFO 2 | LFO 1 | enum |
| Pitch LFO Dep | 0–12 | 0.05 | st |
| Pitch LFO Mod | Off / On | Off | bool |
| Pitch Env | Off / Env 1 / Env 2 | Off | enum |
| Pitch Env Dep | −12 to +12 | 0 | st |
| Pitch Env Mod | Off / On | Off | bool |
| Pitch Wheel | 0–12 | 2.0 | st |
| PWM LFO | Off / LFO 1 / LFO 2 | Off | enum |
| PWM LFO Dep | 0–0.5 | 0 | linear |
| PWM Env | Off / Env 1 / Env 2 | Off | enum |
| PWM Env Dep | −0.5 to +0.5 | 0 | linear |
| Cutoff LFO1 Dep | 0–48 | 0 | st |
| Cutoff LFO2 Dep | 0–48 | 0 | st |
| Cutoff Env Dep | −96 to +96 | 0 | st |
| Vel→Cutoff | −96 to +96 | 0 | st |
| Wheel→PWM | −0.5 to +0.5 | 0 | linear |
| Wheel→Cutoff | −96 to +96 | 0 | st |
| Wheel→Reso | 0–1 | 0 | linear |
| Wheel→X-Mod | −48 to +48 | 0 | st |
