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

- [x] Reported latency is 0 with the filter off, and equals the measured
  base-rate group delay of the interp+decimate round-trip with it on, at each
  F ∈ {1,2,4,8}.
  *(`latency_extension_reports_filter_group_delay` smoke test: 0/16/24/28 for
  enum idx 0/1/2/3.)*
- [x] An impulse rendered through the filter-on path peaks at the reported
  latency offset (± the FIR's symmetric spread) — i.e. the number is honest.
  *(`halfband::tests::impulse_peaks_at_reported_latency`: the round-trip impulse
  peaks within ±2 of `impulse_at + roundtrip_latency_base_samples(F)`.)*
- [x] Toggling `filter-enable` or changing `filter-oversample` emits a
  latency-changed notification to the host.
  *(Audio thread flags a change → `request_callback`; `on_main_thread` issues
  `request_restart` + `latency.changed()`. Main-thread UI/flush edits notify
  directly. Idempotent — a 1× enable or a no-op edit emits nothing.)*
- [x] Latency value is derived from the halfband module's group-delay constants,
  not a separate hardcoded number.
  *(`vxn2_dsp::halfband::roundtrip_latency_base_samples` =
  `2·GROUP_DELAY_OVERSAMPLED·(F−1)/F`; `FilterParams::reported_latency_samples`
  calls it — no second constant.)*

## Implementation notes

- `roundtrip_latency_base_samples(factor)` in `vxn2-dsp/src/halfband.rs` is the
  derived figure. Both the up and down halfband cascades are symmetric; each
  contributes `GROUP_DELAY_OVERSAMPLED·(1−1/F)` base-rate samples (geometric sum
  over `log2 F` stages), so the round-trip is twice that — exact for every
  power-of-two F (0/16/24/28).
- `FilterParams::reported_latency_samples` + `filter_params_of` in
  `vxn2-engine/src/shared.rs`: the seven-param filter decode now lives in one
  place, feeding both the audio render snapshot and the latency report.
- CLAP wiring in `vxn2-clap/src/lib.rs`: registers `PluginLatency`; `get()`
  computes live and records `VxnShared::reported_latency`; the audio thread
  compares the live figure each block / flush and `request_callback`s on drift;
  `on_main_thread` does the `request_restart` + `changed()`. Main-thread param
  flush and the editor timer tick notify directly.

## Notes

The dry-when-off and filtered-when-on paths are mutually exclusive per block, so
internal sample-alignment between them is not required — only the host-visible
PDC figure must be correct. If the UI later shows latency, it reads the same
derived value.
