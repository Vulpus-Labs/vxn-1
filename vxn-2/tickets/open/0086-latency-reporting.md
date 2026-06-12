---
id: "0086"
title: "Host latency reporting for the oversampled filter path"
priority: medium
created: 2026-06-12
epic: E007
depends: ["0084"]
---

## Summary

Seventh ticket of [E007](../../epics/open/E007-optional-per-voice-filter.md).
The up/down halfband cascade has group delay (16 oversampled samples per stage;
`HalfbandFir::GROUP_DELAY_OVERSAMPLED`). The filter-on path therefore adds a
fixed, oversample-factor-dependent latency the dry bypass path does not. VXN2
currently reports `latency: 0` to the host
([vxn2-clap smoke](../../crates/vxn2-clap/tests/smoke.rs#L103)). Report the real
figure so host plugin-delay-compensation stays correct (ADR 0004 §8).

## Design

- Compute the base-rate-referred latency of the interpolate + decimate
  round-trip at the active oversample factor: sum the per-stage group delays of
  both directions for F ∈ {1,2,4,8} (1× = 0) and divide back to base-rate
  samples. Expose a const/fn from the halfband module so the figure is derived,
  not hardcoded twice.
- Report it via the CLAP latency extension: `latency = 0` when
  `filter-enable` is off, the computed value when on.
- Re-report (flush / latency-changed notification per CLAP) on:
  `filter-enable` toggle and `filter-oversample` change. Both are structural
  selectors (not continuous automation), so a latency change on edit is
  acceptable.

## Acceptance criteria

- [ ] Reported latency is 0 with the filter off, and equals the measured
  base-rate group delay of the interp+decimate round-trip with it on, at each
  F ∈ {1,2,4,8}.
- [ ] An impulse rendered through the filter-on path peaks at the reported
  latency offset (± the FIR's symmetric spread) — i.e. the number is honest.
- [ ] Toggling `filter-enable` or changing `filter-oversample` emits a
  latency-changed notification to the host.
- [ ] Latency value is derived from the halfband module's group-delay constants,
  not a separate hardcoded number.

## Notes

The dry-when-off and filtered-when-on paths are mutually exclusive per block, so
internal sample-alignment between them is not required — only the host-visible
PDC figure must be correct. If the UI later shows latency, it reads the same
derived value.
