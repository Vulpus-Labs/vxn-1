---
id: E007
title: Optional per-voice oversampled OTA filter — post-stack/pre-voice, cutoff+reso matrix targets
status: closed
created: 2026-06-12
closed: 2026-06-12
---

## Goal

Add VXN1's OTA-C ladder filter to VXN2 as an *optional*, *per-voice* stage that
sits post-stack-sum / pre-voice-sum, with oversampling localised to the filter
(upsample each voice in, run the ladder at the oversampled rate, defer a single
shared decimation past the voice-sum). Cutoff and resonance become mod matrix
destinations. The filter is off by default and, when off, the render path and
output are bit-identical to today.

When this epic closes: a patch can enable a multimode resonant filter
(LP/HP/BP/Notch × 2/4-pole) per voice, modulate its cutoff and resonance from
the matrix, choose 1×/2×/4×/8× oversampling, and the off path remains the tuned
sample-major hot loop with zero added cost. Design is fixed by
[ADR 0004](../../adrs/0004-optional-per-voice-oversampled-filter.md).

## Scope

**In:**

- Port the OTA-C ladder kernel (`OtaLadderKernel` + coeffs + `FilterMode` /
  `FilterSlope`) and Padé-(5,6) `fast_tanh` into a dependency-free `vxn2-dsp`.
- Port the halfband decimator and **build the missing interpolation stage**
  (VXN1 only ships a decimator).
- Per-stack filter state + stack-major oversampled render body, gated by a
  block-rate `filter-enable` check against the unchanged sample-major bypass.
- Single shared decimation post voice-sum (deferred-decimation optimisation).
- Quiescence-skip per stack (state-magnitude gate, freeze on skip).
- `DestId::Cutoff` / `DestId::Resonance` matrix destinations + the Filter
  parameter section.
- Host latency reporting for the oversampled filter path.
- Filter faceplate panel — controls for every filter param + FX-style enable
  toggle, so the feature ships with UI, not headless.
- Benchmarks + tests: on/off cost, bypass bit-identity, aliasing/THD,
  quiescence, latency.

**Out (later / not this epic):**

- VXN1's Moog `ladder` and standalone `hpf` (OTA HP mode covers high-pass).
- Per-sample coeff ramping (`PolyOtaLadder` SoA sibling) — block-rate frozen
  coeffs only; per-block ramp is an escape hatch only if zipper noise is
  measured.
- Per-lane (intra-stack) cutoff variation — physically precluded by the
  post-lane-fold placement.
- Drive make-up gain / enable-disable level matching (sound-design call).

## Tickets

- [x] [0080 — Port OTA-C ladder kernel + Padé fast_tanh into vxn2-dsp](../../tickets/closed/0080-port-ota-ladder-kernel.md)
- [x] [0081 — Port halfband decimator (Oversampler) into vxn2-dsp](../../tickets/closed/0081-port-halfband-decimator.md)
- [x] [0082 — Build halfband interpolation (upsampling) stage](../../tickets/closed/0082-halfband-interpolator.md)
- [x] [0083 — Filter params + Cutoff/Resonance matrix destinations](../../tickets/closed/0083-filter-params-and-matrix-dests.md)
- [x] [0084 — Per-stack filter state + stack-major oversampled render path, gated bypass](../../tickets/closed/0084-per-stack-filter-render-path.md)
- [x] [0085 — Quiescence-skip per stack + state/coeff-ramp freeze](../../tickets/closed/0085-quiescence-skip.md)
- [x] [0086 — Host latency reporting for the oversampled filter path](../../tickets/closed/0086-latency-reporting.md)
- [x] [0087 — Filter benchmarks + tests: cost, bypass bit-identity, aliasing, quiescence](../../tickets/closed/0087-filter-benches-and-tests.md)
- [x] [0088 — Filter faceplate panel: controls + FX-style enable toggle](../../tickets/closed/0088-filter-faceplate-panel.md)

## Dependency order

```text
0080 (ladder kernel + tanh) ──┐
                              ├─> 0084 (render path) ──> 0085 (quiescence skip)
0081 (decimator) ──> 0082 (interpolator) ──┘            │
                                                         ├─> 0086 (latency)
0083 (params + matrix dests) ──┬───────────────────────┘
                               └─> 0088 (faceplate panel)
0084 + 0085 ──> 0087 (benches + tests)
```

- 0080, 0081, 0083 are independent foundations and can run in parallel.
- 0082 (interpolator) depends on 0081 (it mirrors the decimator's halfband +
  cascade structure).
- 0084 is the integration keystone: it needs the kernel (0080), both resamplers
  (0081 + 0082), and the params/dests (0083) to wire cutoff/reso and the enable
  toggle.
- 0085 layers the skip optimisation onto the working render path.
- 0086 reports the latency the resamplers in 0084 introduce.
- 0087 validates the whole feature once render + skip are in place.
- 0088 (faceplate panel) needs only the params (0083); it builds in parallel
  with the render path and drives inert params until 0084 lands.

## Acceptance

- With `filter-enable = off`, the render path is the current sample-major loop
  and output is **bit-identical** to pre-epic output for every factory patch
  (no OS buffers allocated, no per-sample branch added).
- With the filter on, each of LP/HP/BP/Notch × 2/4-pole produces the expected
  response; resonance self-oscillates at the top of its range without blowing up
  (finite, bounded) at every oversample factor.
- `Cutoff` and `Resonance` route end-to-end through the matrix (velocity →
  cutoff, mod-env → cutoff, etc.) without DC offset; block-rate coeff updates
  produce no audible zipper at musical modulation rates.
- The deferred-decimation path is numerically equivalent (within FIR tolerance)
  to per-voice decimation: a known multi-voice input decimated once-post-sum
  matches summing per-voice-decimated outputs.
- Oversampling measurably reduces aliasing/THD of a driven, resonant sweep
  versus 1× (documented dB figures at 2×/4×/8×).
- Quiescence-skip leaves resonant release tails intact (no clipped ring) yet
  skips truly settled voices; a held chord with released tails costs
  measurably less than without the skip.
- The plugin reports correct host latency when the filter is enabled, updating
  on enable/disable and OS-factor change; `latency: 0` only when off.
- Every filter param is reachable from the faceplate (E003 "every param"
  invariant holds); the enable toggle reads as an optional module like the FX,
  and `Cutoff`/`Resonance` appear as mod-matrix destinations.
- No RT allocations, no `unwrap`/`expect`/panics across the process callback in
  the new path.
- Each ticket's individual acceptance criteria met.
