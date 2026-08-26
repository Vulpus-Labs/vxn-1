---
id: "0309"
product: monorepo
title: "Hoist the CPU meter to vxn-core-web and give VXN1b one"
priority: medium
created: 2026-08-26
epic: E045
depends: ["0291"]
---

## Summary

VXN1b's browser build reports no render load at all. vxn-1 and vxn-2 both show a
CPU badge; VXN1b was ported without one — its coordinator has no `onCpu`, and its
processor never times a quantum.

That matters more here than on either sibling. [E045](../../epics/open/E045-vxn1b-web-wasm-browser-port.md)
lists worst-case performance as an **open question rather than a known-good**:
32 voices across two layers, an oversampled ladder per voice and a full FX chain,
single-threaded wasm with no NEON. The number belongs on screen where a visitor
(and we) can see it, not in a profiler someone has to go and open.

Same shape as [0308](0308-shared-on-screen-piano.md): the badge is ~66 lines of
pure presentation living inside vxn-2's bridge, so it hoists to
`crates/vxn-core-web/assets/cpu-meter.mjs` and both ports import it.

## Design

### The badge is shared; the measurement is not

The badge knows nothing about any synth — it takes `(load, peak)` and draws a
bar. Shared. The *measurement* lives in each port's worklet processor and
coordinator, which are already forked by design (they encode per-synth transport
shape), so VXN1b gets its own copy of ~40 lines: window accumulate → mean → EMA →
peak-with-decay → `port.postMessage({type:"cpu"})`, and the coordinator forwards
it to `onCpu`.

### Windowed, never per-quantum

`performance.now()` is historically absent from `AudioWorkletGlobalScope`, so the
clock may be `Date.now()` at ~1 ms — against a ~2.7 ms quantum budget. A single
quantum's `dt` is then 0 or 1, which is noise, not a reading. Sum over 64 quanta
(~170 ms, ~6 Hz reporting) and the coarse clock converges on the true mean; an
EMA across windows tames what is left. The clock kind is reported once so a
`date` reading is legible as a window mean rather than a precise figure. vxn-1's
original meter fell back to a constant and read 0 everywhere — the fallback is
the bug to avoid.

### Off on Safari, and that is not laziness

Safari's AudioWorklet runs on a one-quantum buffer with no render-thread slack
([[vxn1-web-safari-audioworklet]]). The per-quantum clock reads and the periodic
`postMessage` can themselves cause the glitching the meter exists to reveal, so
the meter is disabled there and reports `null` — which the badge shows as `n/a`,
deliberately distinct from a real 0% and from the initial dash.

The detection must not catch Chrome on iOS: it carries Apple's vendor string but
is Blink and measures fine. Disabling it there would be a permanent, silent
`n/a` on a browser that has no problem.

## Acceptance criteria

- [ ] `crates/vxn-core-web/assets/cpu-meter.mjs` holds `createCpuMeter`; neither
      port has a copy; vxn-2's badge test passes against the shared module.
- [ ] VXN1b's processor times a quantum only when enabled, accumulates over a
      window and posts `{type:"cpu", load, peak}`.
- [ ] VXN1b's coordinator forwards it to `onCpu` and logs the clock kind once.
- [ ] Safari → meter off, worklet told so, `onCpu(null, null)` fired once so the
      badge reads `n/a`. Chrome-on-iOS → meter on.
- [ ] Both bundles ship the module.
- [ ] All three web suites green, 0 skipped.
- [ ] Manual: the badge shows a plausible load that rises with held voices.

## Notes

- The badge's `bottom:102px` clears the on-screen piano's 92px bar (0308). If
  either number changes, they change together.
- Not taken: testing the processor's accumulator directly. It lives inside an
  `AudioWorkletProcessor` subclass in a worklet-only file, and neither sibling
  tests theirs either. The wiring around it IS tested; the arithmetic is not.
  Worth its own harness if the meter ever misreports.
