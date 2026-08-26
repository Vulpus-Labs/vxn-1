---
id: "0312"
product: vxn-1b
title: "The event wire has four halves and two are ceremony — pick one encoder"
priority: high
created: 2026-08-26
epic: E047
depends: ["0309"]
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
- [ ] Node suites green, 0 skipped, under [[0309]]'s CI.

## Notes

- `codec.rs`'s `Event::tag()` / `Event::offset()` plus the encode and decode
  bodies are four hand-maintained 16-arm matches over one enum. Collapsing them
  (hoist `offset: u8` into a `Slot { offset, kind }`, or derive `tag()` from
  `#[repr(u8)]`) is [[0319]]; deleting the encode half here removes two of the
  four regardless.
- The ring's own comments about the byte loop (JSC GC stalls the render thread)
  and the block-writer overflow policy are load-bearing constraints, not
  narration — they survive this ticket and [[0315]].
