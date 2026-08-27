---
id: "0312"
product: vxn-1b
title: "The event wire has four halves and two are ceremony — pick one encoder"
priority: high
created: 2026-08-26
epic: E047
depends: ["0321"]
---

## Summary

Two independent reviews, working from opposite ends of the wire, each found the
encoder on their side unused. Put together:

| half | status |
|---|---|
| JS `EventRing._push` | **ships** — this is what writes bytes |
| Rust `codec.rs::decode` | **ships** — this is what reads them |
| JS `encode` / `encodeInto` / `decodeAt` / `decode` + the `ev` builder table | dead — ~200 of 321 lines |
| Rust `encode` / `encode_into` | dead — and advertises *"The hot-path entry point — no allocation"* |

Consequences, in order of how much they matter:

1. **The golden table validates an encoder that never runs.** The cross-language
   fixture in `event-codec.test.mjs` pins JS `encode` against Rust `decode`. The
   encoder that actually produces every byte the engine sees —
   [`EventRing._push`](../../vxn-1b/crates/vxn1b-wasm/web/event-ring.mjs#L115) —
   is checked only against the JS decoder that also doesn't ship, plus one
   end-to-end note-on in `wasm-agreement.test.mjs`.
2. **Rust's `encode_into` claims a hot path this crate does not have.**
   [codec.rs:275-325](../../vxn-1b/crates/vxn1b-wasm/src/codec.rs#L275-L325).
   Same for `pack_matrix_addr` ([:135](../../vxn-1b/crates/vxn1b-wasm/src/codec.rs#L135))
   and `matrix_field_code` ([:166](../../vxn-1b/crates/vxn1b-wasm/src/codec.rs#L166)):
   production only ever *un*packs.
3. **The slot layout is written out in full three times** — `codec.rs:9-23`,
   `event-codec.mjs:1-10`, and
   [WIRE-FORMAT.md:16-30](../../vxn-1b/crates/vxn1b-wasm/web/WIRE-FORMAT.md#L16-L30)
   — while `event-codec.mjs:9` says *"The slot layout and tags are documented
   once, in `WIRE-FORMAT.md`."*

## Design

The choice is which JS encoder survives, and it is not obvious:

- **Keep `_push`, delete `encode`/`encodeInto`.** Matches reality; smallest
  diff. Cost: the golden table has to be re-expressed against `_push`'s ring
  buffer rather than a returned byte array, which is a slightly clumsier test.
- **Make `_push` call `encodeInto`.** Keeps the golden table pointed at the
  same function it already tests and gives the wire one named encoder. Cost: a
  call through a module boundary in the ring's write path — this runs on the UI
  thread at gesture rate, not in the worklet, so the cost is nominal.

**Prefer the second.** The golden table is the artifact with the most value
here, and it should test the shipping path without contortion; `_push`'s
argument order can then also stop disagreeing with `WebHost.setMatrix` (see the
`offset` note below).

Either way: delete the JS decode half outright (Rust decodes), demote Rust's
encode half to `#[cfg(test)]` or delete it, and drop the "hot-path entry point"
claim. Replace the two duplicated layout tables with a pointer to
`WIRE-FORMAT.md`, as that file already claims.

### Fold in: the count handshake

[event-codec.mjs:38-41](../../vxn-1b/crates/vxn1b-wasm/web/event-codec.mjs#L38-L41)
hand-declares `PATCH_COUNT = 75` / `GLOBAL_COUNT = 35`, but the only guard —
`wasm-agreement.test.mjs:54` and the controller handshake — checks
`TOTAL_PARAMS` alone. Compensating drift (+1 patch, −2 global) passes the guard
and computes wrong ids in `patchClapId` / `globalClapId`. Export
`vxn1b_patch_count()` / `vxn1b_global_count()` from
[host.rs](../../vxn-1b/crates/vxn1b-wasm/src/host.rs) and assert all three.

### Fold in: the `offset` argument order

`_push(type, offset, paramIdx, value, note, flag)` takes six positional numbers
and puts `offset` **first**
([`pushMatrixEdit(offset, layer, slot, field, value)`](../../vxn-1b/crates/vxn1b-wasm/web/event-ring.mjs#L184)),
while `WebHost.setMatrix(layer, slot, field, value, offset = 0)`
([coordinator.mjs:406](../../vxn-1b/crates/vxn1b-wasm/web/coordinator.mjs#L406))
puts it **last**. One convention, across both layers.

## Acceptance criteria

- [ ] Exactly one encoder and one decoder on the wire; the dead halves are gone
      or `#[cfg(test)]`-gated with a reason.
- [ ] The golden table exercises the encoder that ships. **Say so in the
      close-out** — the test goes green afterwards but is proving something
      different from what it proved before, and a future reader should not
      assume continuity.
- [ ] No function advertises a hot path it is not on.
- [ ] The slot layout appears in full in exactly one place; the two code files
      link to it.
- [ ] The handshake asserts patch count, global count and total — not just the
      sum — and a deliberately drifted pair fails it. Verify once by hand.
- [ ] `offset` sits in the same position in both layers' signatures.
- [ ] Node suites green, 0 skipped, under [[0321]]'s CI.

## Notes

- `codec.rs`'s `Event::tag()` / `Event::offset()` plus the encode and decode
  bodies are four hand-maintained 16-arm matches over one enum. Collapsing them
  (hoist `offset: u8` into a `Slot { offset, kind }`, or derive `tag()` from
  `#[repr(u8)]`) is [[0319]]; deleting the encode half here removes two of the
  four regardless.
- The ring's own comments about the byte loop (JSC GC stalls the render thread)
  and the block-writer overflow policy are load-bearing constraints, not
  narration — they survive this ticket and [[0315]].

## Close-out (2026-08-27)

- **One encoder, one decoder.** JS encodes, Rust decodes, and neither has a
  counterpart. `EventRing._push` now takes a built `ev.*` event and writes the
  slot through
  [`encodeInto`](../../vxn-1b/crates/vxn1b-wasm/web/event-codec.mjs#L126) —
  option 2 from the Design section — stamping `seq` over the zero the codec
  leaves at off 10, before it advances the write index (so an unknown tag
  throws without publishing). The JS `encode` wrapper, `decodeAt`, `decode` and
  the module's decode half are deleted; `grep 'decodeAt|export function decode'`
  over the wire files returns nothing.
- **The golden table now exercises the encoder that ships — and it is proving
  something different from what it proved before.** Before 0312 it drove a JS
  `encode` that nothing called, checked against a JS `decode` that nothing
  called either, while every byte the worklet saw came from `_push`, which no
  table touched. It now drives `encodeInto` at a non-zero `base`, the way the
  ring does. A green run here is **not continuity** with the old green run: the
  old one was compatible with `_push` being wrong. The cross-language check
  against `codec.rs::tests::golden` is unchanged, and there is a new
  `encodeInto overwrites all 16 bytes` case — the ring reuses slots, so a field
  the codec skips would carry the previous event's bytes round the wrap.
- **Rust's encode half is `#[cfg(test)]` with the reason in a section banner**
  ([codec.rs:261](../../vxn-1b/crates/vxn1b-wasm/src/codec.rs#L261)): `encode`,
  `encode_into`, `put_u16`, `put_f32`, and with them `Event::tag` /
  `Event::offset` — `host.rs`'s slice loop reads tag and offset out of the raw
  slot bytes, so nothing shipping calls either. `pack_matrix_addr` and
  `matrix_field_code` likewise: production only ever *un*packs, and they exist
  to prove `unpack_matrix_addr` against the packing it inverts. That removes
  two of the four hand-maintained 16-arm matches the Notes flagged for
  [[0319]]; decode and apply remain.
- **No function advertises a hot path it is not on.** The `encode_into` claim
  is gone; `grep -rni 'hot.path' vxn-1b/crates/vxn1b-wasm/` leaves only
  `controller.mjs`'s UI-event banner, which is a path that ships.
- **The slot layout is written out in full in exactly one place.** `grep -rl
  'off 0  u8'` over `vxn-1b/` → `WIRE-FORMAT.md` alone. `codec.rs`'s module doc
  and `event-codec.mjs`'s header both link to it, as the latter already
  claimed. `event-codec.mjs`'s param-id block was also de-literalised — it
  stated `PATCH_COUNT = 75` two lines above the `export const` that declares
  it; it now states the ranges as formulae in P and G.
- **`readSlot` is not a third half.**
  [event-ring.mjs:230](../../vxn-1b/crates/vxn1b-wasm/web/event-ring.mjs#L230)
  reports a slot's raw fields with no switch on the tag — the inverse of the
  ring's framing, not of the codec's. `drainInto` (the documented debug drain)
  and the ring's own tests are its only callers; it replaced the `decode` calls
  in `event-ring.test.mjs`'s raw-drain assertions.
- **The count handshake checks all three.** New exports
  [`vxn1b_patch_count`](../../vxn-1b/crates/vxn1b-wasm/src/host.rs#L188) /
  [`vxn1b_global_count`](../../vxn-1b/crates/vxn1b-wasm/src/host.rs#L194) and
  [`vxnc_global_count`](../../vxn-1b/crates/vxn1b-web-controller/src/lib.rs#L789),
  sourced from the engine via new `codec::PATCH_COUNT` / `codec::GLOBAL_COUNT`
  re-exports. Asserted in `wasm-agreement.test.mjs` (against the built artifact)
  and in `WebController.instantiate`'s boot handshake, plus
  `codec::tests::total_params_matches_the_engine` and
  `vxn1b-web-controller`'s `total_params_agrees_with_the_engine`.
- **Drift verified by hand.** Set `PATCH_COUNT = 76` / `GLOBAL_COUNT = 33` in
  `event-codec.mjs`: `TOTAL_PARAMS` still computes 185, so the old total-only
  guard passes. Both new guards fail — `wasm-agreement` with `patch count:
  … drifted from the engine`, and every controller-backed suite with
  `controller PATCH_COUNT 75 != JS mirror 76 — param layout drift` (16 failures
  in `controller.test.mjs` alone). Restored and re-ran green.
- **`offset` sits in one position at all three layers** — `ev.*`,
  `EventRing.push*`, `WebHost.*`: the event's own fields, then `offset`, then
  `channel`, the last two defaulted. It used to lead on the ring
  (`_push(type, offset, …)`, `pushMatrixEdit(offset, layer, …)`) and trail on
  `WebHost.setMatrix`. All 15 coordinator call sites and every test producer
  updated.
- **Suites.** `cargo test --workspace`: 1364 pass, 0 fail. `node --test
  vxn-1b/crates/vxn1b-wasm/web/*.test.mjs`: **148 pass, 0 skipped** — down from
  151 because the three JS decode tests (`decode of golden bytes`, `round-trips
  through encode -> decode`, `unknown and reserved tags decode to null`) went
  with the decoder; their contract is covered by `codec.rs`'s
  `decode_of_golden_bytes_yields_the_event`, `every_event_round_trips` and
  `unknown_and_reserved_tags_decode_to_none`. The count in
  [test.yml](../../.github/workflows/test.yml#L69)'s 0321 narrative was
  reworded off the literal so it does not rot again.
- **No DAW pass needed.** Behaviour-preserving by construction: the bytes on
  the wire are unchanged (the golden table is the proof), and nothing in the
  render path moved.
