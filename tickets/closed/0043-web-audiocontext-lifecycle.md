---
id: "0043"
product: vxn-1
title: "AudioContext lifecycle: autoplay unlock, suspend/resume, device change, teardown"
priority: medium
created: 2026-06-15
epic: E016
depends: ["0042"]
---

## Summary

The main-thread AudioContext lifecycle around the 0042 coordinator: gate the
first boot on a user gesture (autoplay policy), suspend/resume cleanly, handle
output-device changes, and tear down without leaks. The main-thread complement
to 0040's worklet-side lifecycle.

## Design

- **Autoplay unlock.** An AudioContext starts `suspended` until a user gesture.
  The page must gate construction/`resume()` on a click (or keydown) and reach
  `running` cleanly; show the gate state in the UI hook (a "Start audio" button
  for now, faceplate-driven in E018).
- **Suspend/resume.** On `AudioContext.statechange` (tab backgrounded, manual
  suspend), stop driving the ring and `resume()` back to rendering without
  dropping transport state or leaving stuck notes — pairs with the worklet's
  0040 `reset()` guard. Decide + document who flushes sounding voices on resume.
- **Device change.** Handle `navigator.mediaDevices.ondevicechange` / context
  `sinkId` changes (where supported): re-route output without rebuilding the
  whole graph, or rebuild + re-share the same SABs if the sample rate changes
  (the engine rebuilds at the new rate via the 0040 `sampleRate` path).
- **Teardown.** `close()` the context, detach the node, drop the SAB refs, send
  the worklet `destroy` (0040) — no leaked voices, nodes, or dangling SABs
  across a teardown/rebuild cycle.

## Acceptance criteria

- [ ] Audio does not start until a user gesture; after it, the context reaches
      `running` and stays live.
- [ ] Suspend then resume restarts rendering with no stuck notes and intact
      transport (ring/store) state.
- [ ] An output-device or context sample-rate change is handled without a dead
      context — output continues (engine rebuilt at the new rate if needed).
- [ ] Teardown closes the context and releases the node + SABs with no leaks;
      a fresh boot afterwards works.

## Notes

- Depends on [0042](0042-web-main-thread-coordinator.md) (the coordinator whose
  lifecycle this manages).
- Mirrors the 0040 worklet lifecycle on the main-thread side; the two together
  are the full lifecycle story (worklet render-thread + context owner).
- Out of scope: the worklet-side lifecycle (0040, done); persistence of state
  across reloads (E019).

## Close-out (2026-06-15)

- AudioContext lifecycle state machine layered onto the 0042 `WebHost` in
  [coordinator.mjs](../../vxn-1/crates/vxn-wasm/web/coordinator.mjs): `gateState`
  (`idle → starting → running → suspended → closed`) with an `onState` observer
  the UI renders from. `start()` is the autoplay-unlock entry (gesture-driven,
  context starts `suspended` → `running`). Verified by `coordinator-lifecycle.test.mjs`
  ("gate starts idle and reaches running only after start()").
- Suspend/resume via an `AudioContext` `statechange` listener (+ programmatic
  `suspend()`/`resume()`). **Resume voice-flush:** the main thread posts the
  worklet `reset` (0040) on resume so a long suspend can't leave stuck notes,
  WITHOUT touching the ring/store — transport state survives. Verified
  ("suspend then resume flushes voices and keeps transport", "flush once",
  "no flush from non-suspended").
- Device change: `devicechange` listener; `setSink(sinkId)` re-routes via
  `setSinkId` without rebuilding (false where unsupported); `rebuild()` tears
  down + re-boots over the SAME SABs for a sample-rate change (engine rebuilds
  at the new rate via the 0040 `sampleRate` path). Verified ("setSink re-routes
  without rebuilding", "rebuild makes a new context over the same SABs").
- Teardown: `teardown()` (alias `dispose()` for 0042 back-compat) posts worklet
  `destroy` (0040), detaches listeners + node, closes the context, drops SAB
  refs; a fresh `WebHost` boots clean afterward. Verified ("teardown closes...
  drops SAB refs", "a fresh WebHost boots cleanly after teardown").
- Generated page [xtask/src/main.rs](../../vxn-1/xtask/src/main.rs) `web_index_html()`
  renders off the gate hook with a Stop/teardown button. Full suite: 12/12
  lifecycle + 6 event-codec + 1 param-store node tests pass; 0042/0040 harnesses
  still green. Browser-event wiring (real gesture/statechange/devicechange/
  `setSinkId`) needs manual DAW-less browser confirmation; the logic those events
  drive is unit-covered via a mock AudioContext.
