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

- [x] `event-ring.mjs`: SPSC ring over SAB, 16-byte slots, producer
      (main) / consumer (worklet) — ported from vxn-1. Added `pushEvent` so
      the full codec vocabulary (norm params, gestures) can ride the ring, not
      just the byte pushers. Dropped vxn-1's JS `renderQuantumSliced` /
      `applyRecord` — the vxn-2 production path drains raw bytes into the wasm
      host, which owns the slice loop (0153).
- [x] `param-store.mjs`: lock-free param store over SAB, flat 209-param space
      (TOTAL_PARAMS imported from the codec, not hardcoded); `writeBulk` /
      `readAll` / `pollDiffs` readback pump + `applyStoreToHost` block-fold.
- [x] `event-codec.mjs`: JS encode/decode of the 16-byte slot, byte-identical
      to the Rust codec (0153) — proven by a golden table that is a literal copy
      of `codec.rs tests::golden` (same offsets, tags, norm flag).
- [x] Shared one-page wire-format spec `web/WIRE-FORMAT.md`, referenced from
      both `src/codec.rs` and `web/event-codec.mjs` headers.

## Close-out (2026-07-10)

Done. `node --test` over the three `.test.mjs` → **18 pass**. Files under
`vxn-2/crates/vxn2-wasm/web/`: `event-codec.mjs`, `event-ring.mjs`,
`param-store.mjs` + a `.test.mjs` each, plus `WIRE-FORMAT.md`.

Divergences from the vxn-1 port (all match the 0153 codec):

- **Flat param space, 209.** No Upper/Lower layer split → no
  `PATCH_COUNT`/`GLOBAL_COUNT`/`patchClapId`/`globalClapId`. `TOTAL_PARAMS` is
  the single declared constant (JS) / re-checked against `vxn2_engine::TOTAL_PARAMS`
  (Rust). Ticket's "~250+" estimate was high — it's 209.
- **No key-mode / split-point** (tags 7/8 reserved-unused); a JS test asserts
  both decode to `null`.
- Tests use `node:test` (like vxn-1's transport), runnable headless with
  `node --test vxn-2/crates/vxn2-wasm/web/*.test.mjs`; independent of the
  `vxn2-ui-web` vitest suite.

Consumed by the worklet (0156) and produced by the bridge (0157) / input
adapters (0160).

## Notes

vxn-1 froze this format in spike 0035; vxn-2 carries the byte layout verbatim
(minus tags 7/8) so the two synths share a wire format.
