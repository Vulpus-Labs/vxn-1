---
id: "0043"
product: vxn-2
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
