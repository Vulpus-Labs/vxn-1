---
id: "0088"
product: vxn-2
title: "Glitch/xrun stress + latency / block-size tuning"
priority: high
created: 2026-06-22
epic: E020
depends: ["0087"]
---

## Summary

Second ticket of [E020](../../epics/open/E020-web-perf-crossbrowser-ship.md).
With the 0087 rig proving steady-state render cost, this ticket proves the port
survives *bursty* load without xruns: sustained 16-voice chords plus a stream of
param automation plus FX tails, played live in the browser, watching for audio
dropouts. Where headroom is tight it tunes the only knobs the worklet exposes —
the AudioContext latency hint and any host-side lookahead — and folds in the
known **Safari one-quantum-buffer** limit
(`vxn1-web-safari-audioworklet`).

## Design

- **Stress driver.** Reuse the 0087 worst-case bench patch
  ([bench.rs](../../vxn-1/crates/vxn-wasm/src/bench.rs)) but drive it through the
  *production* transport, not the bench rig: notes + param automation written to
  the 0035 SAB event ring, consumed by the real worklet host
  ([host.rs render loop](../../vxn-1/crates/vxn-wasm/src/host.rs#L69)). The
  glitch signal is the worklet's own render-load meter
  ([vxn-processor-0038.js:101-119](../../vxn-1/crates/vxn-wasm/web/vxn-processor-0038.js#L101))
  going over budget plus audible dropouts; the meter already posts `load`/`peak`.
- **Latency knobs.** The only levers without engine changes are (a) the
  `AudioContext({ latencyHint })` chosen at boot and (b) the SAB ring depth /
  drain policy. The render quantum is fixed at 128 by the platform
  ([lib.rs QUANTUM = 128](../../vxn-1/crates/vxn-wasm/src/lib.rs#L29)) — Web Audio
  does not let us pick a larger render block, so "block-size tuning" here means
  the latencyHint and how many quanta of slack the graph buffers, not the render
  quantum itself.
- **Safari.** The CPU meter is already disabled on Safari
  (`processorOptions.cpuMeter=false`,
  [vxn-processor-0038.js:39-41](../../vxn-1/crates/vxn-wasm/web/vxn-processor-0038.js#L39))
  because Safari ships a one-quantum buffer and ignores `latencyHint`; document
  that as a platform floor, not a bug to fix, and record the worst-case voice
  count Safari sustains.

## Acceptance criteria

- [ ] (headless) A node stress harness writes a sustained note+automation stream
      to the ring and asserts the host renders every quantum without panicking
      and without producing NaN/Inf (drains a full ring in one render).
- [ ] (MANUAL, M1 Chrome) Play a sustained 16-voice chord with live param
      automation + full FX tails for ≥60 s; record any xruns/dropouts and the
      render-load meter's steady + peak readings.
- [ ] (MANUAL) Sweep `latencyHint` (`interactive` vs `playback` vs an explicit
      seconds value); record the lowest-latency setting that stays glitch-free at
      16 voices, and document it as the default.
- [ ] (MANUAL, Safari) Record the max glitch-free voice count under Safari's
      one-quantum buffer; document the fallback (e.g. reduced default poly) for
      the 0091 matrix.

## Notes

- Depends on 0087: this ticket needs the worst-case patch and a measured
  steady-state cost before it can attribute glitches to bursts vs baseline.
- Memory: `vxn1-web-safari-audioworklet` (Safari one-quantum buffer, ignores
  latencyHint, meter-off fix already shipped).
- Out of scope: denormal cliffs (0089), the published matrix (0091).
