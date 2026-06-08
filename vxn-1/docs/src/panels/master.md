# Master

Global, instrument-wide controls. All parameters here are shared across both layers.

## Tuning and level

- **Master Tune** (−12 to +12 st) — instrument-wide pitch transposition. Applies after per-layer tuning, including pitch modulation.
- **Master Volume** (0–1, default 0.7) — final output gain, per-sample smoothed to avoid zipper noise.

## Drift

**Master Drift** (0–1, default 0) introduces small per-voice oscillator phase offsets, modelling analogue tuning instability. At 0 every voice's oscillator starts in phase with the others (clinical); at 1 voices are heavily decorrelated (broader stereo image, less coherent transients).

Drift is sampled once per voice allocation, so a given note's drift stays constant for that note's lifetime — repeated notes get fresh drift values.

## Limiter

**Limiter** (Off / On, default Off) inserts a brick-wall limiter at the master output. Useful as a final safety net for heavy patches; in normal use, leave it off and manage your master level with the **Master Volume** knob.

The limiter is post-volume, so cranking Master Volume into the limiter is a valid way to push hot levels with a hard ceiling.

## Oversampling

**Oversample** (Off / 2× / 4× / 8×, default 2×) sets the synthesis oversampling factor. The per-voice path (oscillators, sub, noise, cross-mod, filter, drive saturation) runs at the oversampled rate; effects run at host rate.

| Mode | When to use |
| --- | --- |
| **Off** (1×) | CPU constrained; willing to live with aliasing on sync / ring / aggressive filter sweeps. |
| **2×** | Default. Adequate for most patches. |
| **4×** | Bright sync leads, ring modulation with non-sine sources, heavy resonance. |
| **8×** | Worst-case anti-aliasing; clean PM with non-sine modulators. CPU-heavy. |

Changing the oversample setting is real-time safe — no clicks, no allocations, no reload required.

## Parameters

| Parameter | Range | Default | Unit | Notes |
| --- | --- | --- | --- | --- |
| Master Tune | −12 to +12 | 0 | st | |
| Master Volume | 0–1 | 0.7 | linear | Per-sample smoothed |
| Master Drift | 0–1 | 0 | linear | Per-voice phase offset amount |
| Limiter | Off / On | Off | bool | Brick-wall limiter on master bus |
| Oversample | Off / 2× / 4× / 8× | 2× | enum | Synthesis oversampling factor |
