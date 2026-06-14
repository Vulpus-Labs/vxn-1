---
id: "0039"
product: vxn-2
title: "Scaffold: cross-thread param store + audio→main diff readback"
priority: high
created: 2026-06-14
epic: E015
depends: ["0036"]
---

## Summary

Implement the cross-thread parameter store chosen in the
[0036](0036-web-controller-placement-adr.md) ADR — the web analogue of
`SharedParams` — plus the audio→main **param-diff readback** that lets the
UI see audio-thread param drift (host-automation-style writes). The
readback ports the timer-tick param-diff pump
([vxn-clap/src/lib.rs:193-236](../../vxn-1/crates/vxn-clap/src/lib.rs#L193-L236)).

## Design

Per the 0036 decision, one of:

- **SAB-backed atomic array** (preferred if 0036 picks it): a
  `SharedArrayBuffer` of 165 atomics indexed by CLAP id (69×2 patch + 27
  global). Worklet reads lock-free in the render loop; the controller
  writes on edits. Latest-value-wins semantics, matching `SharedParams`.
- **Param-events-on-the-ring**: param changes flow as 0037 records; the
  store is the engine's own `ParamValues`. Simpler, but the bulk
  preset-load case (165 at once) and the diff readback need care.

**Diff readback (either way)**: a path for the worklet to publish current
param values back to the main thread (a second SAB region the main thread
polls on rAF, or a return ring), so the main thread can diff against
`last_seen` and emit `ParamChanged` to the UI — exactly what the plugin's
pump does for host automation. This is what makes automation and
modulation visible in the UI.

## Acceptance criteria

- [ ] The 0036-selected store is implemented: the worklet reads params
      lock-free in the render loop; the controller/main thread writes.
- [ ] A bulk update (load a preset → 165 params) applies correctly and
      without glitching the audio path.
- [ ] An audio-thread param write is observable from the main thread via
      the diff readback, yielding a `ParamChanged`-equivalent — verified
      by a test that mutates a param on the audio side and sees it surface.
- [ ] Param addressing matches 0036 (CLAP-id layout) and the 0037 codec.

## Notes

- Depends on [0036](0036-web-controller-placement-adr.md) (decides the
  mechanism). Proceeds alongside [0038](0038-web-worklet-audio-host.md)
  (which reads the store).
- Reference: `vxn-engine/src/shared.rs` (`SharedParams`), the param-diff
  pump (cited above). Related: [[vxn1-id-stability-dropped]].
- The readback is what E018's UI bridge consumes to reflect automation —
  keep its shape compatible with `ViewEvent::ParamChanged`.
- Out of scope: the UI side of the readback (E018), preset storage (E019).
