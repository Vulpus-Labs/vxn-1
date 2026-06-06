---
id: "0002"
title: vxn-core-utils — FTZ, smoother, note utils, host-sync
priority: high
created: 2026-06-06
epic: E001
---

## Summary

Extract the trivially-shared utility surface that today lives in
duplicated form across vxn-1 and vxn-2: denormal/FTZ guard, one-pole
parameter smoother, MIDI note → Hz, and the host-tempo subdivision
table (BPM ↔ Hz / beats / seconds). Zero dependencies, `no_std`
where practical.

## Acceptance criteria

- [ ] `vxn_core_utils::ftz::ScopedFlushToZero` — RAII guard, sets
      FTZ + DAZ on x86 (MXCSR), FZ on ARM (FPSR). Drop restores
      prior state. Mirrors vxn-1's `vxn_dsp::ScopedFlushToZero` and
      vxn-2's `vxn2_engine::ftz` line-for-line (pick the more recent
      of the two as the source of truth — the bit patterns must
      match an existing real-time-tested impl).
- [ ] `vxn_core_utils::smoothing::Smoothed` — one-pole low-pass
      param smoother. `new(sample_rate, time_ms)`, `set_target`,
      `tick`, `current`. Coefficient computed once at construction;
      `set_sample_rate` triggers re-derive. Matches vxn-1's
      `vxn_dsp::smoothing::Smoothed` API.
- [ ] `vxn_core_utils::smoothing::one_pole_coeff(time_ms, sample_rate) -> f32`
      — exposed for crates that need the raw coefficient without
      the `Smoothed` wrapper (vxn-2's `smoother.rs` uses this form).
- [ ] `vxn_core_utils::midi::note_to_hz(note: u8) -> f32` — A440
      reference, MIDI note 0 = 8.176 Hz. Match vxn-1's impl exactly.
- [ ] `vxn_core_utils::sync` — host-tempo subdivision table from
      vxn-1's `vxn_app::sync`. `Subdivision` enum (1/1, 1/2, 1/2T,
      1/2D, ..., 1/64), `subdivision_label(s)`, `subdivision_hz(s, bpm)`,
      `subdivision_seconds(s, bpm)`. Used by LFO sync + delay sync.
- [ ] Crate is `no_std` compatible with default features. `std`
      feature enables `alloc`-using helpers if any (none expected
      in this scope).
- [ ] Zero external deps in default features.
- [ ] Unit tests: FTZ guard round-trips MXCSR/FPSR state; `Smoothed`
      reaches target within `5 * tau` to within 1%; `note_to_hz(69) ==
      440.0`; subdivision_hz(`1/4`, 120 bpm) == 2.0.
- [ ] Doc-comments on every pub item. No further docs.

## Notes

The FTZ impl is the most sensitive part — both existing impls were
verified against real-host audio (no denormal stalls under sustained
release tails). Diff the two before copying; if they differ on a bit
pattern, the more recent one wins, with the older's reasoning
captured in a `// Why:` comment.

Subdivision label format must round-trip with vxn-1's preset JSON
("1/4", "1/4T", "1/4D"). Do not "improve" the labels.

This is the cheapest of the four extraction tickets. Land it first
to validate the workspace plumbing from 0001 before sinking time
into 0003–0005.
