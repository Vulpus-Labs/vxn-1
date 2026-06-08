# Modulation routes

Modulation in VXN1 is organised by **destination**: each routable destination is its own panel with source selectors and depth knobs. There is no matrix. Every route on this page is always live — no routing menus, no slots to assign.

## Pitch modulation

Routes to **both oscillators** simultaneously (vibrato, pitch envelope, pitch wheel).

- **Pitch LFO** — source: Off / LFO 1 / LFO 2.
- **Pitch LFO Dep** (0–12 st, exp taper) — vibrato depth.
- **Pitch LFO Mod** — when on, the LFO route is suppressed by host automation (lets you put vibrato on a performance hardware controller without DAW automation overriding it).
- **Pitch Env** — source: Off / Env 1 / Env 2.
- **Pitch Env Dep** (−12 to +12 st) — envelope-driven pitch sweep. Negative inverts.
- **Pitch Env Mod** — same automation suppression as the LFO route.
- **Pitch Wheel** (0–12 st) — pitch-bend range from MIDI pitch wheel.

The pitch route applies to **both oscillators together**. If you want one oscillator to bend while the other stays put, detune them statically and route only one to a destination — but at the cost of using the cross-mod sweep, since pitch routes don't have a per-osc selector.

## PWM modulation

Routes to **Osc 1 and Osc 2 pulse widths** simultaneously (so PWM only affects oscillators currently set to Pulse).

- **PWM LFO** — source: Off / LFO 1 / LFO 2.
- **PWM LFO Dep** (0 to 0.5) — sweep depth (0.25 = full ±25% PW excursion).
- **PWM Env** — source: Off / Env 1 / Env 2.
- **PWM Env Dep** (−0.5 to +0.5) — envelope depth, can invert.

Static PW from each oscillator's PW knob is the centre point; modulation displaces from there.

## Filter modulation

Four fixed depths, **no source selectors**. Every depth knob is live simultaneously; set unused routes to zero.

- **Cutoff LFO1 Dep** (0–48 st) — LFO 1 → cutoff.
- **Cutoff LFO2 Dep** (0–48 st) — LFO 2 → cutoff.
- **Cutoff Env Dep** (−96 to +96 st) — Env 1 → cutoff. Negative *closes* the filter on note-on.
- **Vel→Cutoff** (−96 to +96 st) — MIDI velocity → cutoff.

The 96-semitone range gives enough headroom to fully sweep from minimum to maximum cutoff via velocity alone — useful for dynamic playing where soft notes are dark and hard notes are bright.

Key tracking is binary on/off on the [Filter panel](filter.md#key-track), not a continuous modulation depth.

## Cross-Mod Sweep

Wide-range pitch modulation of Osc 2, gated on the Cross-Mod Type being non-Off. Used with Sync or PM to drive dramatic timbral sweeps.

- **X-Mod Sweep Env** — source: Off / Env 1 / Env 2.
- **X-Mod Sweep Env Dep** (−48 to +48 st) — envelope depth.
- **Wheel→X-Mod Sweep** (−48 to +48 st) — Mod Wheel → cross-mod sweep.

With Cross-Mod Type set to Off, all sweep knobs are inert.

## Mod Wheel routes

The Mod Wheel (MIDI CC1) has four fixed destinations. Set any to zero to disable that route.

- **Wheel→PWM** (−0.5 to +0.5) — pulse-width depth.
- **Wheel→Cutoff** (−96 to +96 st) — cutoff depth.
- **Wheel→Reso** (0–1) — adds to resonance.
- **Wheel→X-Mod Sweep** (−48 to +48 st) — wide cross-mod pitch sweep.

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
| X-Mod Sweep Env | Off / Env 1 / Env 2 | Off | enum |
| X-Mod Sweep Env Dep | −48 to +48 | 0 | st |
| Wheel→PWM | −0.5 to +0.5 | 0 | linear |
| Wheel→Cutoff | −96 to +96 | 0 | st |
| Wheel→Reso | 0–1 | 0 | linear |
| Wheel→X-Mod Sweep | −48 to +48 | 0 | st |
