---
id: "0155"
product: vxn-2
title: vxn-2 SAB transport JS — event-ring, param-store, event-codec
priority: medium
created: 2026-06-30
epic: E030
---

## Summary

The SharedArrayBuffer transport between main thread and AudioWorklet:
an SPSC event ring (note + param events) and a lock-free param store
(block-start folding), plus the JS side of the 16-byte event codec. Ports
`vxn-wasm/web/event-ring.mjs`, `param-store.mjs`, `event-codec.mjs`,
sized for vxn-2's larger param surface.

## Acceptance criteria

- [ ] `event-ring.mjs`: SPSC ring over SAB, 16-byte slots, producer
      (main) / consumer (worklet) — ported from vxn-1.
- [ ] `param-store.mjs`: lock-free param store over SAB sized for vxn-2's
      ~250+ params; producer writes, consumer polls diffs.
- [ ] `event-codec.mjs`: JS encode/decode of the 16-byte slot, byte-
      identical to the Rust codec in ticket 0153 (same field offsets, type
      tags, norm flag).
- [ ] A shared one-page wire-format spec (comment or doc) referenced by
      both the Rust and JS codec so they can't drift.

## Notes

vxn-1 froze this format in spike 0035. The only vxn-2 change is param-store
sizing — re-derive slot count from the vxn-2 param table, don't hardcode
165. Consumed by the worklet (0156) and produced by the bridge (0157) /
input adapters (0160).
