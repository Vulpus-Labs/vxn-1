---
id: "0289"
product: vxn-1b
title: "AudioWorklet + coordinator bootstrap — the audio graph and its lifecycle"
priority: medium
created: 2026-08-25
epic: E045
depends: ["0287", "0288"]
---

## Summary

Fifth ticket of [E045](../../epics/open/E045-vxn1b-web-wasm-browser-port.md):
the thing that actually makes sound in a browser. Four modules under
`vxn-1b/crates/vxn1b-wasm/web/`:

- `audio-host.mjs` — the steady-state render loop: fold the param store, drain
  the ring into wasm, one `vxn1b_host_render` call, copy the output out, publish
  telemetry.
- `host-runner.mjs` — lifecycle and failure policy around it: instantiate from
  bytes, silence-until-ready, reset, teardown, and render-thread trap safety.
- `vxn1b-processor.js` — the `AudioWorkletProcessor` shell.
- `coordinator.mjs` — the main-thread `WebHost`: AudioContext, worklet load, SAB
  allocation, autoplay-gesture gate, suspend/resume, device change, and the
  producer surface the UI drives.

Ports vxn-1's four. The split of concerns carries over unchanged: the audio host
is the render loop, the runner is the policy, the processor is the shell, the
coordinator is the main-thread half.

## Design

### Recovering from a render-thread trap

A wasm trap poisons the instance, so the runner catches it at the worklet
boundary, outputs silence, and re-instantiates over the **same SABs** — the
ring's read/write indices and the param store live there, so a fresh host resumes
where the dead one left off, and params come back on the first fold because the
worklet-side mirror is NaN-seeded.

**VXN1b has more to lose than vxn-1 does.** vxn-1's only non-automatable state is
a key mode and a split point — two bytes, which its runner shadows and re-applies
on re-instantiate. VXN1b's is key state (mode, split point, LFO 2 link), **the
whole per-layer matrix topology**, the scope tap, and the tempo. None of it is in
the param store, and none of it will be re-sent by a ring that has already
delivered it.

Shadowing all of that on the audio thread would mean decoding every record the
runner currently copies as opaque bytes, and keeping a second copy of the
topology that can drift from the engine's. The controller already holds the
authoritative model, so the replay belongs there: the processor posts `trap`, the
coordinator surfaces it, and the controller re-broadcasts. This ticket wires the
signal and leaves a **loud default** — an unhandled trap warns rather than
failing silently — with the actual re-broadcast landing in
[0290](0290-vxn1b-faceplate-rewire.md) alongside the rest of the controller
bridge. Until then a trap costs routing, which is worth knowing rather than
papering over.

### Telemetry

The runner owns the `TelemetryWriter` (0288) and ticks it after each successful
render, so the rate division counts *rendered* quanta rather than wall time. A
trap therefore pauses telemetry rather than publishing frames from a dead engine.

### Trap tests without a trap export

vxn-1 added a `vxn_host_force_trap` export purely so the recovery path could be
exercised. VXN1b does not: the catch is JS-level, so a fake exports object whose
`vxn1b_host_render` throws proves the same boundary without putting a
test-only function in the shipped ABI.

## Acceptance criteria

- [ ] `audio-host.mjs` renders a quantum: store fold, raw ring drain, one wasm
      render call, output copy — with memory views cached and re-derived only
      when the wasm buffer identity changes.
- [ ] Steady-state render allocates nothing (no per-quantum view construction).
- [ ] `host-runner.mjs` outputs silence before ready and buffers nothing that
      the ring already carries.
- [ ] A trap is caught, output goes silent, `onTrap` fires, and a re-instantiate
      over the same SABs restores audio — proven with a throwing fake.
- [ ] Telemetry ticks only on rendered quanta.
- [ ] `coordinator.mjs`: allocates the three SABs, seeds the param store from
      engine defaults before the worklet starts, gates on a user gesture,
      and exposes the producer surface (notes with channel, params, bend, wheel,
      pressure, key state, matrix edits, scope tap, tempo).
- [ ] Seeding happens **before** the worklet's first fold — an unseeded store
      would fold zeros over every param and silence the instrument.
- [ ] suspend/resume, teardown and rebuild-over-the-same-SABs covered.
- [ ] Web suite green, 0 skipped.

## Notes

- Reference: `vxn-1/crates/vxn-wasm/web/{audio-host,host-runner,coordinator}.mjs`
  and `vxn-processor.js`.
- `vxn1b_host_render(host, n)` takes no key-mode/split args — unlike vxn-1's,
  that state rides the ring (0287).
- Out of scope: the faceplate rewire and the controller wasm (0290), the xtask
  bundle (0291). Nothing here fetches `factory.bin`.
- Blocks 0290.

## Close-out (2026-08-25)

- Four modules: [audio-host.mjs](../../vxn-1b/crates/vxn1b-wasm/web/audio-host.mjs),
  [host-runner.mjs](../../vxn-1b/crates/vxn1b-wasm/web/host-runner.mjs),
  [vxn1b-processor.js](../../vxn-1b/crates/vxn1b-wasm/web/vxn1b-processor.js),
  [coordinator.mjs](../../vxn-1b/crates/vxn1b-wasm/web/coordinator.mjs).
- Render loop: store fold → raw ring drain → one `vxn1b_host_render(host, n)` →
  output copy → telemetry tick. No key-mode/split arguments — that state rides
  the ring, so the coordinator has no latched shared state to replay.
- Steady state allocates nothing:
  `the steady-state render does not rebuild its memory views` holds all three
  cached views identical across 32 quanta.
- Silence before ready, and a note pushed pre-ready is not lost (the ring's read
  index is untouched until the worklet drains).
- Trap policy proven with a throwing fake rather than a force-trap export in the
  shipped ABI: output goes silent, nothing escapes `process()`, `onTrap` fires,
  and the runner re-instantiates over the same SABs so a later note still sounds.
  The default handler is asserted LOUD — `the default trap handler is loud rather
  than silent` requires a warning naming the re-broadcast consequence.
- Coordinator: three SABs, gate machine (idle → starting → running → suspended →
  closed), suspend/resume with the resume-only voice flush, rebuild over the same
  SABs, teardown, and the full producer surface incl. MPE channel, matrix edits,
  scope tap and tempo.
- Seeding-before-fold verified from the failure side: with a zeroed store the
  same note is silent, and with defaults seeded it sounds — which is why
  `start()` seeds before the node is constructed.
- **Bug found by testing rebuild()'s documented behaviour**: `rebuild()` calls
  `start()`, which seeded the store unconditionally, so rebuilding reset every
  param the user had touched — despite `rebuild()` existing so the patch survives
  a sample-rate change. Seeding is now once per `WebHost` (`_storeSeeded`). vxn-1
  and vxn-2 have the same bug; filed as [[0296]] rather than fixed here.
- Web suite 80/80, 0 skipped.
- Trap-recovery re-broadcast is deliberately left to 0290, which owns the
  controller; the signal and a loud default are wired here.
