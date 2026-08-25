---
id: "0287"
product: vxn-1b
title: "vxn1b SAB transport JS — event ring, param store, codec twin + WIRE-FORMAT"
priority: medium
created: 2026-08-25
epic: E045
depends: ["0286"]
---

## Summary

Third ticket of [E045](../../epics/open/E045-vxn1b-web-wasm-browser-port.md): the
JS side of the main→worklet transport. Three modules under
`vxn-1b/crates/vxn1b-wasm/web/`, plus the wire-format doc that makes the Rust and
JS halves one contract rather than two:

- `event-codec.mjs` — the JS twin of
  [codec.rs](../../vxn-1b/crates/vxn1b-wasm/src/codec.rs). Its golden byte table
  replicates the Rust one and must match byte-for-byte.
- `event-ring.mjs` — the lock-free SPSC ring over a `SharedArrayBuffer`: main
  thread produces, worklet drains once per quantum.
- `param-store.mjs` — the current-value param SAB (main→audio) plus the
  readback region (audio→main) the controller's diff pump reads.
- `WIRE-FORMAT.md` — the slot layout, the tag table, and what is deliberately
  *not* shared between the three synths.

Ports vxn-1's versions. vxn-1's are the right base, not vxn-2's: VXN1b's param
space is **two-layer** (`2 * PATCH_COUNT + GLOBAL_COUNT`, 75/35/185) exactly like
vxn-1's, where vxn-2 flattened its layout and dropped `patchClapId` /
`globalClapId` along with it.

## Design

### The mirror is the risk

[0285](0285-web-param-mirror-drift.md) killed both existing browser builds by
letting a hand-declared JS param count drift behind its engine. This ticket
creates VXN1b's copy of exactly that constant, so it inherits exactly that risk.

Mitigations, in order of how much they're worth:

1. `vxn1b_total_params()` is already exported from the wasm (0286) and the
   controller handshake asserts the JS mirror against it at instantiate — the
   check that *caught* 0285, just never run.
2. `WIRE-FORMAT.md` states the counts once, next to the instruction for
   updating them, rather than leaving the reader to find three files.
3. The suite must not skip its own coverage. vxn-2's wasm-backed tests are
   `{ skip: !HAVE }` on an artifact path that the *other* port's `xtask web`
   deletes, which is how 11 real failures hid behind a green run. VXN1b's tests
   build against `target/wasm32-unknown-unknown/**` — the crate's own artifact,
   which nothing else clobbers — and **fail** rather than skip when it is
   missing, naming the build command.

### Tag numbering is shared only up to 10

Worth writing down because it is not obvious and the code currently implies more
than it should: tags **1–10 are common** across vxn-1, vxn-2 and VXN1b, and that
is the whole of the guarantee. Tags 11+ are synth-local and already conflict —
vxn-2 uses 11 for `matrix_row` and 12 for `patch_swap`, VXN1b uses 11 for
`lfo2_link` and 12 for `matrix_edit`. Each synth's ring is its own; nothing
crosses. `WIRE-FORMAT.md` says so explicitly so nobody ports a tag by number.

### What VXN1b does not carry

Checked against vxn-2's prior art rather than assumed:

- **No `patch_swap` pulse.** vxn-2 needs one because its native host bumps a
  `load_epoch` on preset load to silence the outgoing patch's ringing voices, and
  the worklet's separate `SharedParams` never sees it. VXN1b has no epoch
  mechanism at all — its native preset load swaps params and topology and lets
  voices ring — so adding a pulse would make the browser quieter than the plugin.
- **No bulk state event.** `Engine::load_state` beyond params is just the
  keyboard record (tags 7/8/11) and `apply_envelopes()`, and
  `Synth::set_param` already re-cooks envelopes via `recooks_envelopes(id)`. A
  param stream is therefore equivalent to a state load, which is what makes the
  store-fold path faithful.

## Acceptance criteria

- [ ] `event-codec.mjs` encodes/decodes all 16 live tags; its golden table is
      byte-identical to `codec.rs`'s, asserted row by row.
- [ ] Unknown and reserved tags (incl. 6) decode to `null`.
- [ ] Matrix-address packing round-trips against the Rust `pack_matrix_addr`
      layout, incl. the out-of-range rejections.
- [ ] `event-ring.mjs`: SPSC push/drain, power-of-two capacity, wrap, and
      overflow behaviour covered; slot writes stay 16 bytes.
- [ ] `param-store.mjs`: two-layer `LAYOUT` (`UPPER_BASE` / `LOWER_BASE` /
      `GLOBAL_BASE`), bulk write, per-slot atomics, NaN-seeded first poll
      broadcasts every id, readback diff pump.
- [ ] Declared counts match the engine: a test reads `vxn1b_total_params()` out
      of the built wasm and asserts the JS mirror agrees.
- [ ] That test **fails loudly** when the wasm is missing — it never skips.
- [ ] `WIRE-FORMAT.md` documents the slot, the tag table, the 1–10-only sharing
      rule, and how to update the counts.
- [ ] `node --test vxn-1b/crates/vxn1b-wasm/web/*.test.mjs` green, 0 skipped.

## Notes

- Reference: `vxn-1/crates/vxn-wasm/web/{event-codec,event-ring,param-store}.mjs`
  and `vxn-2/crates/vxn2-wasm/web/WIRE-FORMAT.md`.
- The six shared modules from [0284](0284-vxn-core-web-shared-browser-glue.md)
  are **not** re-forked here; VXN1b picks them up in 0292/0294.
- Out of scope: the worklet + coordinator (0289), the faceplate rewire (0290),
  and the meter/scope return channel (0288).
- Blocks 0288 and 0289.
