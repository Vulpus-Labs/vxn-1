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

- [x] `vxn2-processor.js` AudioWorkletProcessor: instantiates the engine
      wasm via the shared `host-runner.mjs` + `audio-host.mjs`, drains the ring
      into the `vxn_host_events_ptr` scratch, calls `vxn_host_render(host, n)`
      (no key-mode/split args), copies L/R out. Registers as
      `vxn2-host-processor`.
- [x] `coordinator.mjs` (`WebHost`): creates AudioContext, allocates event-ring
      + param-store SABs, adds the worklet module, hands it the wasm bytes,
      seeds the store from engine defaults, exposes the producer API
      (note/pitchBend/modWheel/sustain over the ring; setParam(s) over the
      store; pollParamDiffs readback).
- [x] AudioContext lifecycle: autoplay unlock (idle→starting→running),
      suspend/resume (posts worklet a `reset` voice-flush, only on resume),
      sample-rate change via `rebuild()` over the same SABs, device change /
      setSink, teardown/dispose. 11 mock-context lifecycle tests.
- [x] Worklet trap safety: a render-thread panic is caught at the runner
      boundary → silence + `onTrap` + async re-instantiate over the same SABs;
      proven end-to-end against the real wasm.
- [~] Served page reaches "audio live" after a click and renders a test tone:
      the headless proxy is green — `host-runner.test.mjs` boots the REAL
      `vxn2_wasm.wasm`, pushes a note through the ring and asserts audible
      output (silence-until-ready + tone + trap recovery). The actual served
      page needs 0157 (`index.html` bridge) + 0158 (xtask `--serve` with
      COOP/COEP); the in-browser click-to-live check rides those.

## Close-out (2026-07-10)

Done (bar the browser-served click, which needs 0157/0158 to have a page to
serve). Files under `vxn-2/crates/vxn2-wasm/web/`: `vxn2-processor.js`,
`host-runner.mjs`, `audio-host.mjs`, `coordinator.mjs` +
`coordinator-lifecycle.test.mjs` (11 mock tests) and `host-runner.test.mjs`
(2 real-wasm tests, auto-skip if the artifact isn't built). `node --test` →
13 pass.

vxn-2 divergences from the vxn-1 port: no key-mode/split shared state on the
worklet port or in the runner/host; `vxn_host_render` takes `(host, n)`; the
param fold pushes via `vxn_host_set_param` (block-start, `applyStoreToHost`),
since vxn-2 folds the store into the engine inside `vxn_host_render`; names are
`vxn2-host-processor` / `vxn2_wasm.wasm`.

## Notes

References: `vxn-wasm/web/{coordinator,host-runner,vxn-processor,audio-host}`.
Depends on 0153 (engine exports) + 0155 (transport). 128-frame quantum,
CONTROL_BLOCK sub-chunking owned by the wasm host.
