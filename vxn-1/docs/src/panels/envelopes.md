# Envelopes

Each voice has two independent ADSR envelopes.

- **Env 1** — the **modulation envelope**. Default destinations: filter cutoff, pitch envelope, PWM envelope, cross-mod sweep. Never wired to the VCA.
- **Env 2** — the **amplitude envelope**. Hardwired to the VCA. Can additionally route to pitch / PWM / cross-mod sweep (via the source selectors on those panels), but its role at the VCA is non-negotiable.

Both envelopes have the same parameter shape: A / D / S / R + Shape.

## Stages

- **Attack** (A) — time from gate-on to peak. 0.001–10 s, exponential taper centred at 1 s.
- **Decay** (D) — time from peak to sustain level. Same taper.
- **Sustain** (S) — held level (0–1, linear) until gate-off.
- **Release** (R) — time from gate-off to silence. Same exponential taper.

## Shape

The **Shape** parameter picks between linear and exponential segments:

- **Linear** — each stage interpolates straight from start value to end. Constant rate of change.
- **Exponential** — each stage curves: rapid initial motion, asymptotic approach to target. Smoother on the ear, especially for the amplitude envelope.

Env 1 defaults to **Linear** (predictable for modulation). Env 2 defaults to **Exponential** (smoother for amplitude).

## Amp Gate

The **Amp Gate** parameter (0/1) bypasses Env 2 at the VCA. With Amp Gate on, the amp envelope is replaced by a hard gate that follows note-on / note-off — useful for organ-like sounds or when you want to drive amplitude entirely from an LFO-to-amp modulation rather than the envelope.

## Tremolo

Independent of Env 2, the **Amp LFO** route applies a tremolo to the VCA stage:

- **Amp LFO** — source selector (Off / LFO 1 / LFO 2).
- **Amp LFO Dep** — tremolo depth (0–1).

This route is additive on top of Env 2 (or the Amp Gate, if active).

## Parameters — Env 1

| Parameter | Range | Default | Unit | Notes |
| --- | --- | --- | --- | --- |
| Env 1 Attack | 0.001–10 | 0.005 | s | Exp taper |
| Env 1 Decay | 0.001–10 | 0.3 | s | Exp taper |
| Env 1 Sustain | 0–1 | 0 | linear | |
| Env 1 Release | 0.001–10 | 0.3 | s | Exp taper |
| Env 1 Shape | Linear / Exp | Linear | enum | |

## Parameters — Env 2

| Parameter | Range | Default | Unit | Notes |
| --- | --- | --- | --- | --- |
| Env 2 Attack | 0.001–10 | 0.005 | s | |
| Env 2 Decay | 0.001–10 | 0.2 | s | |
| Env 2 Sustain | 0–1 | 0.8 | linear | |
| Env 2 Release | 0.001–10 | 0.3 | s | |
| Env 2 Shape | Linear / Exp | Exp | enum | |
| Amp Gate | Off / On | Off | bool | Bypass Env 2 at VCA |
| Amp LFO | Off / LFO 1 / LFO 2 | Off | enum | Tremolo source |
| Amp LFO Dep | 0–1 | 0 | linear | Tremolo depth |
