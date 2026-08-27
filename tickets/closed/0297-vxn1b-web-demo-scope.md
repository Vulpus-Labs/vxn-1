---
id: "0297"
product: vxn-1b
title: "Trim the VXN1b web port to demo scope — drop the plugin-grade robustness machinery"
priority: medium
created: 2026-08-25
epic: E045
depends: ["0289"]
---

## Summary

The VXN1b browser build is a **demo**, not a product. Its failure story is "reload
the page", and it does not need to survive the things a plugin has to survive
inside somebody's DAW for eight hours.

[0289](../closed/0289-vxn1b-worklet-coordinator.md) ported vxn-1's coordinator
and runner wholesale, and with them a set of mechanisms that exist only because
vxn-1's web port was aiming at plugin parity. None of it is load-bearing for a
demo, and some of it is actively worse than doing nothing.

## What goes, and why

- **`rebuild()` + sample-rate change.** An AudioContext's rate is immutable, so
  vxn-1 tears the graph down and re-boots over the same SABs when the default
  device moves to different hardware. That is a real scenario for a plugin and a
  page-reload for a demo. Removing it also removes the bug
  [0296](0296-web-rebuild-resets-params.md) describes, on this port.
- **Device-change listening and `setSink()`.** Same reasoning: following a device
  switch in place is nice for an instrument you leave open all day, and irrelevant
  for a page you opened to try a synth.
- **`setSampleRate` on the runner, the host, and the C ABI.** Only reachable from
  the two above. `vxn1b_host_set_sample_rate` was already a rebuild-the-engine
  stub because `Engine` has no in-place setter — dropping it removes an export
  whose only honest implementation was "throw the engine away".
- **Trap re-instantiation.** This one is not merely unnecessary — it is
  *misleading*. 0289 documented that a rebuilt engine loses every piece of
  non-automatable state: key mode, split point, LFO 2 link, the whole per-layer
  matrix topology, scope tap and tempo. So the recovery path restores *audio*
  while silently restoring the *wrong patch*. For a demo, "the sound stopped,
  reload" is strictly better than "the sound came back, routed differently, with
  nothing on screen to say so". Keep the catch — a trap must not escape
  `process()` and wedge the context — and keep the loud `onTrap`; drop the
  automatic rebuild behind it.
- **The readback region, `pollDiffs`, `newLastSeen`, `publishReadback`.** Already
  established as dead on this port: nothing originates a param value outside the
  controller, and the worklet publishes back exactly what it read. It halves the
  param SAB and deletes the `norm`/`display` stubs nobody will now fill.

## What stays

Not everything inherited is plugin-grade. These are browser facts, not DAW ones:

- **The gesture gate.** Autoplay policy requires the context resume to happen
  inside a user-gesture call stack. Without it there is no sound at all.
- **suspend/resume mirroring and the resume voice-flush.** Tabs get backgrounded
  constantly and browsers suspend the context on their own; without the flush a
  demo comes back from a background with stuck notes. Routine, not exotic.
- **The trap catch** (see above), silence-until-ready, and teardown.
- **The telemetry seqlock.** ~20 lines that stop the scope trace tearing
  visibly — a demo is mostly *looked at*.
- **The ring's block-writer policy** — inherent to the transport, not robustness
  bolted on.

## Acceptance criteria

- [ ] `rebuild()`, `setSink()`, the device-change listener and the `mediaDevices`
      option are gone from `coordinator.mjs`.
- [ ] `setSampleRate` is gone from `coordinator.mjs`, `host-runner.mjs`,
      `audio-host.mjs`, the processor's port switch, and the C ABI
      (`vxn1b_host_set_sample_rate`).
- [ ] A render trap still goes silent, still does not escape `process()`, and
      still reports — but does **not** re-instantiate.
- [ ] The readback half of the param SAB is gone; `STORE_BYTES` halves and a test
      asserts the new size.
- [ ] `WIRE-FORMAT.md` updated where it describes the removed pieces.
- [ ] Tests for the removed behaviour are deleted, not left asserting nothing.
- [ ] Web suite green, 0 skipped; `cargo test -p vxn1b-wasm` green.

## Notes

- 0290's re-broadcast-on-trap requirement is dropped as a consequence: there is
  no automatic recovery to re-broadcast into.
- [[0296]] still stands for vxn-1 and vxn-2, which keep their `rebuild()`.
- The demo posture should carry into the rest of E045 — notably 0293
  (persistence) and 0294 (ship): a demo wants a working instrument and a
  shareable link, not a durability story.

## Close-out (2026-08-27)

- `rebuild()`, `setSink()`, the `devicechange` listener and the `mediaDevices`
  option are gone from `coordinator.mjs` — the only survivors are the deliberate
  "why this is absent" notes at
  [coordinator.mjs:31-33](../../vxn-1b/crates/vxn1b-wasm/web/coordinator.mjs#L31-L33)
  and [:224](../../vxn-1b/crates/vxn1b-wasm/web/coordinator.mjs#L224).
- `setSampleRate` is gone from all four JS sites and from the C ABI: grep for
  `vxn1b_host_set_sample_rate` over `vxn-1b/` now returns nothing. The Rust
  export is absent from [host.rs](../../vxn-1b/crates/vxn1b-wasm/src/host.rs)
  (`vxn1b_host_set_param` and `vxn1b_host_reset` remain).
- A render trap still goes silent, still does not escape `process()`, still
  reports, and no longer re-instantiates — `host-runner.test.mjs`, *"a render
  trap goes silent, reports, and does not throw out of process()"*.
- Readback region gone: `STORE_BYTES == TOTAL_PARAMS * 4` asserted at
  [param-store.test.mjs:44](../../vxn-1b/crates/vxn1b-wasm/web/param-store.test.mjs#L44),
  with `createParamSAB().byteLength` pinned to it.
- `WIRE-FORMAT.md` records the removal and the reason (no host in a browser) at
  [:103](../../vxn-1b/crates/vxn1b-wasm/web/WIRE-FORMAT.md#L103).
- Closing sweep found one vestige: `host-runner.test.mjs` still stubbed
  `vxn1b_host_set_sample_rate` on its fake wasm, for an export that no longer
  exists. Deleted; the suite is 9/9.
- VXN1b web suite 151 passed / 0 skipped; `cargo test --workspace` 1622 passed,
  0 failed.
