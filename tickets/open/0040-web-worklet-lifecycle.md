---
id: "0040"
product: vxn-2
title: "Harden: worklet lifecycle, sample-rate, trap safety"
priority: medium
created: 2026-06-14
epic: E015
depends: ["0038"]
---

## Summary

Harden the worklet audio-host (0038) for real-world use — the lifecycle
and failure-mode work that turns a working render loop into a dependable
one. Closes the audio-thread half of
[E015](../../epics/open/E015-web-event-driven-core.md).

## Design

- **Instantiate-from-bytes + silence-until-ready**: the worklet receives
  wasm bytes (via `processorOptions`, per 0034), instantiates async, and
  outputs silence until live — already prototyped in 0034's
  `vxn-processor.js`; make it robust (buffer events arriving pre-ready,
  per the spike's `pendingNotes`).
- **Sample rate**: build the engine at the worklet's `sampleRate` global,
  and handle context sample-rate differences (the engine's
  `Synth::set_sample_rate` exists; wire it).
- **Suspend/resume**: cleanly stop/restart rendering with AudioContext
  state changes without dropping ring state or leaking voices.
- **Teardown**: free the engine (`vxn_destroy` analogue), detach the node,
  release the SAB references — no leaks across re-init.
- **Trap safety**: a wasm trap/panic in `process()` must not permanently
  silence the context. Catch at the worklet boundary; recover (re-init) or
  fail loud to the main thread. The plugin unwinds at the host boundary
  ([[vxn1-architecture]] panic policy); the web needs an equivalent.

## Acceptance criteria

- [ ] The worklet outputs silence until the wasm is live, buffering any
      events that arrive first, then applying them in order.
- [ ] The engine runs at the context sample rate; a sample-rate change is
      handled without artefacts.
- [ ] Suspend/resume and teardown/re-init leave no leaked voices, stuck
      notes, or dangling SAB references.
- [ ] A forced trap in `process()` does not permanently kill audio — it is
      caught at the worklet boundary and surfaced to the main thread.

## Notes

- Depends on [0038](0038-web-worklet-audio-host.md) (the host being
  hardened). Last E015 ticket — when this closes, the event-driven core is
  complete and E016/E017/E018 build on a stable contract.
- Reuses 0034's lifecycle scaffolding in
  `vxn-1/crates/vxn-wasm/web/vxn-processor.js` as the starting point.
- Out of scope: the main-thread AudioContext lifecycle (E016) — this
  ticket is the worklet side only.
