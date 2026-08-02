---
id: "0233"
product: vxn-2
title: "SpanDelay move + OsRegion type with unit-tested decimator-continuity invariant"
priority: medium
created: 2026-08-02
epic: E042
depends: ["0226"]
---

## Summary

First ticket of [E042](../../epics/open/E042-oversampled-region.md). Extract
the *mechanics* of vxn-2's oversampled span into `vxn-core-dsp::os_region`:

- `SpanDelay`
  ([engine.rs:292](../../vxn-2/crates/vxn2-engine/src/engine.rs#L292)) — pure
  move; fixed integer stereo delay of exactly the span's roundtrip latency so
  engage/disengage never steps the plugin's group delay.
- `OsRegion { factor, decim: Oversampler, delay: SpanDelay, os bus, fade }` —
  owns the **shared** half of a span: single decimator, latency bridge,
  raised-cosine fade countdown, OS scratch buffers. Deliberately does NOT own
  the in-span kernels, the span FSM, or the interpolators — interpolator
  placement is leg-dependent host policy (per-stack when Filtered, one global
  interpolator over the summed mix when DynOnly), which is exactly what
  vxn-2's topology requires.

## Acceptance criteria

- [ ] `SpanDelay` + `OsRegion` in vxn-core-dsp with API: `bus()`,
      `decimate_into()`, `bridge_dry()`, `begin_fade(len, to_os)`,
      `fade_weight()`, `reset()`, rate accessor via 0226 newtypes.
- [ ] Decimator-continuity rule unit-tested as an invariant: decimator state
      resets **only** when the span's engaged state flips
      (`prev == Bypassed && next != Bypassed`), never on an in-span kernel
      toggle — the documented "engage clunk" fix.
- [ ] Doc comment covers: decimation linearity (decimate(Σ) ≡ Σ decimate, per
      [ADR 0004 §4](../../vxn-2/adrs/0004-optional-per-voice-oversampled-filter.md)),
      SpanDelay constant-latency rationale, why interpolators are excluded.
- [ ] Pure addition + move: vxn-2 render hash unchanged.

## Notes

The reusable unit is "OS region containing N kernels", not "oversampled
filter" — dynamics-inside-span becomes vocabulary, not a special case.
Related: [[vxn2-filter-epic]].
