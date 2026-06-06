---
id: "0021"
title: Pre-FX cleanup filters (HPF + LPF)
priority: medium
created: 2026-06-06
epic: E001
---

## Summary

Insert a fixed one-pole HPF and one-pole LPF on the stereo bus between
the stack mix and the delay input. Cleanup-only: DC block / sub-rumble
removal at the bottom, gentle anti-imaging at the top. No user
controls, no params, no modulation. Placed once on the post-sum bus,
not per-voice (cleanup is global tone-shaping, not a per-note target).

The HPF subsumes a separate DC blocker — a true one-pole HPF has a
zero at DC, so any cutoff > 0 Hz drives DC gain to exactly 0.

## Acceptance criteria

- [ ] `vxn2-dsp::cleanup::CleanupFilter` — stereo, two one-pole
      stages per channel (HPF then LPF). Fixed cutoffs hard-coded:
      HPF at 20 Hz, LPF at 18 kHz. No setters beyond `new(sample_rate)`
      and `reset()`.
- [ ] HPF form: `y = x - x_prev + a * y_prev` (zero at z=1 → DC gain
      exactly 0). LPF form: `y = b * x + (1 - b) * y_prev`.
      Coefficients computed once from sample rate at construction.
- [ ] Re-derive coefficients on sample-rate change via a fresh
      `CleanupFilter::new(sample_rate)` from `Engine::new` /
      `prepare_to_play`. No runtime sample-rate flag.
- [ ] Engine integration: a `CleanupFilter` field on the engine
      alongside `delay` / `reverb` / `master`. Process order becomes
      stack mix → cleanup → delay → reverb → master. Reset on
      `Engine::reset()`.
- [ ] No params, no `EngineParams` entry, no CLAP id, no mod-matrix
      target, no `SharedParams` mirror.
- [ ] Bench: `cleanup_steady` over a 4 s sine burst. Expected cost
      ~negligible (4 mul-add per sample stereo). Numbers go in the
      commit message.
- [ ] Unit test: feed a 1 s DC ramp through the filter; assert mean
      of the last 100 ms of output is within 1e-5 of zero (DC
      removed). Feed a 100 Hz sine at 0 dBFS; assert RMS of output
      is within 0.5 dB of input RMS (passband flat through the
      audible midrange).

## Notes

Why pre-FX, not per-voice: 16 stacks × 8 lanes = 128 lanes. A
per-lane filter would tick 128× per sample for zero modulation
benefit. Pre-FX is one stereo stage, two mul-adds per side per
sample. Same audible result for cleanup; ~128× cheaper.

Why pre-delay, not post-master: feeding rumble or 20 kHz sidebands
into the delay feedback path or reverb FDN compounds them on every
tap / iteration. Cleanup belongs upstream of the spatial FX, not
downstream.

Why not a steeper slope: one-pole is enough for the role —
sub-audible rumble removal and a gentle top-end roll-off. Steeper
filters cost more and start coloring the audible band. The DX7's
analog reconstruction filter was similarly mild and is part of why
its top end sounds the way it does.

Why no `cleanup_on` bypass: the cost is small enough that toggling
it adds branching for no real CPU return, and bypassing it would
let DC + ultrasonic content into the FX chain — which is the exact
thing the filter exists to prevent.
