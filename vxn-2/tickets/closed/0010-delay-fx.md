---
id: "0010"
title: Delay FX (BPM sync, ping-pong)
priority: medium
created: 2026-06-05
epic: E001
---

## Summary

Stereo delay line. BPM-syncable to subdivisions, feedback path, wet/dry mix,
ping-pong toggle. "Clean" — no tape, no filter on the feedback path beyond a
soft DC blocker. Character lives in the synth, not the FX (ADR §7).

## Acceptance criteria

- [ ] Stereo `DelayLine` with two independent delay-line buffers (L and R).
      Buffer size sufficient for max delay time (4000 ms at 96 kHz =
      384k samples per side). Allocated once; no realloc on parameter change.
- [ ] `delay_time` mapped to delay length in samples. When `delay_sync` is
      on, snap to the BPM-subdivision table (reuse VXN1's
      `vxn_app::sync::subdivisions` directly).
- [ ] Feedback path: `out_sample → buffer[next_write]` mixed with input.
      Feedback capped at 0.95 (parameter range) to prevent runaway.
- [ ] DC blocker (single one-pole highpass at ~10 Hz) on the feedback path,
      no other filtering.
- [ ] Ping-pong: when on, L input feeds R buffer with delay, R input feeds
      L buffer with delay (full crossfeed). When off, each side is
      independent.
- [ ] Wet/dry mix: `out = (1 - mix) × dry + mix × wet`. Equal-gain crossfade
      at 0.5.
- [ ] Bypass (`delay_on = false`): pass-through, zero CPU on the delay
      kernel itself. Bypass output is bit-identical to input.
- [ ] Smoothing: delay time changes glide over ~100 ms to avoid pitch
      artefacts (changing read position abruptly = pitch-shift click).
- [ ] Bench: `delay_steady` (active) and `delay_bypassed`.

## Notes

The 384k buffer is a one-time allocation at engine init, sized for the
sample rate × max delay. Re-allocate on sample-rate change only.

Smoothed delay time: linear interpolation between fractional sample
positions, smoothed over ~100 ms when the parameter changes. This is what
makes BPM tempo changes glide smoothly instead of clicking.

The subdivision table from VXN1 includes dotted and triplet variants; reuse
verbatim so the user experience is consistent across both synths.

DC blocker is not for the dry path — only the feedback path. Otherwise
dry/wet mix offsets accumulate.
