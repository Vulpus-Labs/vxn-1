---
id: "0075"
product: vxn-2
title: "Factory presets: stacked + decorrelated square and saw"
priority: medium
created: 2026-06-19
epic: E023
---

## Summary

Ship factory presets that build bandlimited square and saw from algorithm 32
(six carriers), stack them, and decorrelate the lanes for a supersaw. These
are the demonstration of the E023 additive idea: analytic shapes (clean via
0073's Nyquist fade) summed from per-operator phase + ratio (0074), fattened
by the existing per-lane stack decorrelation.

## Acceptance criteria

- [ ] A **square** preset: algo 32, carriers at odd ratios (1,3,5,7,9,11),
      level ~`1/n`, all phase 0. Bandlimited by construction + 0073 fade.
- [ ] A **saw** preset: algo 32, carriers at ratios (1,2,3,4,5,6), level
      ~`1/n`, even harmonics phase-flipped π via 0074 so the time-domain
      shape is an actual saw.
- [ ] Both presets stacked at meaningful density with `StackParams.phase`
      decorrelation set for a supersaw (verify width responds).
- [ ] Presets load via the existing factory bank (include_dir, name-keyed
      sparse TOML — see vxn2-preset-system) and appear in the browser.
- [ ] Documented as the analytic-shape demo; note the six-partial ceiling and
      that spectral fill comes from density, not partial count.

## Acceptance verification

- [ ] Manual listen in a DAW (per verify-audio-in-reaper): square and saw
      read as their named shapes; supersaw is wide and not flangy; no audible
      aliasing on an upward sweep.

## Notes

- Depends on **0073** (clean sweep) and **0074** (saw phase flip). Square
  alone would work without 0074, but ship both together.
- Six carriers = at most six partials → mellow, no high bite solo. Lean on
  stack density for spectral fill; don't expect a bright analog saw from one
  voice. Flag in the preset description.
- Amplitudes `1/n` give the textbook rolloff; tune by ear — a slightly
  brighter-than-`1/n` saw may read better given only six partials.

## Implementation status (code complete; manual listen pending)

- Two factory presets under `presets/factory/Lead/`:
  - **Analytic Square** — algo 32, ratios 1,3,5,7,9,11 at ~1/n levels, all
    phase 0 (square is phase-deaf). Density 8, detune 14 ct, spread 0.70,
    phase 0.65.
  - **Analytic Saw Supersaw** — algo 32, ratios 1-6 at ~1/n, even harmonics
    (ops 2/4/6) phase 0.5 = π for a true saw shape. Density 8, detune 18 ct,
    spread 0.80, phase 0.70.
  - Both: EG held flat (`eg-l2 = eg-l3 = 99`) so the level ratios alone set the
    spectrum; `voice-spread → opN-pan` matrix routes for stereo width; light
    reverb. Comments flag the six-partial ceiling and that fill comes from
    density.
- Load verified by `factory_store_loads_every_preset` /
  `covers_multiple_categories`. **Sound/scope/anti-alias-sweep verification is
  manual in a DAW** (per verify-audio-in-reaper) — pending.
- Depends on 0073 (clean sweep) + 0074 (saw phase flip), both code-complete.
