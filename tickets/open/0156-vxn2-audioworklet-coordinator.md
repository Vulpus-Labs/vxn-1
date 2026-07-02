---
id: "0156"
product: vxn-2
title: vxn-2 AudioWorklet + coordinator bootstrap
priority: medium
created: 2026-06-30
epic: E030
---

## Summary

Main-thread coordinator + AudioWorklet that host the `vxn2-wasm` engine:
create the AudioContext, allocate the SABs, load the worklet, instantiate
the engine wasm from bytes, and run the per-quantum render loop draining
the event ring (ticket 0155). Ports `vxn-wasm/web/coordinator.mjs`,
`vxn-processor.js`, `host-runner.mjs`, `audio-host.mjs`.

## Acceptance criteria

- [ ] `vxn2-processor.js` AudioWorkletProcessor: instantiates the engine
      wasm, drains the ring into `vxn_host_events_ptr` scratch, calls
      `vxn_host_render`, copies L/R out to the worklet outputs.
- [ ] `coordinator.mjs`: creates AudioContext, allocates event-ring +
      param-store SABs, adds the worklet module, hands it the wasm bytes,
      exposes a producer API to the bridge.
- [ ] AudioContext lifecycle: user-gesture autoplay unlock, suspend/resume
      (posts worklet a reset to clear stuck voices), sample-rate change,
      teardown.
- [ ] Worklet trap safety: a render-thread panic surfaces to the main
      thread (onTrap), audio goes silent rather than wedging.
- [ ] Served page reaches "audio live" after a click and renders a test
      tone driven through the ring.

## Notes

References: `vxn-wasm/web/coordinator.mjs` (0042), `host-runner.mjs`
(0040), `vxn-processor.js`, `audio-host.mjs`. Depends on 0153 (engine
exports) + 0155 (transport). 128-frame quantum, same as vxn-1.
