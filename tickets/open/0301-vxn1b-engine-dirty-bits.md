---
id: "0301"
product: vxn-1b
title: "vxn1b-engine: dirty bits on SharedParams (values, matrix, key state)"
priority: medium
created: 2026-08-25
epic: E046
depends: ["0299", "0300"]
---

## Summary

Third ticket of [E046](../../epics/open/E046-dirty-bitset-pump-vxn1-vxn1b.md):
the model half. Add the view-bound change channel to
[`vxn1b-engine::SharedParams`](../../vxn-1b/crates/vxn1b-engine/src/shared.rs#L60-L65),
which today carries only `reload` and `key_dirty` — neither of which the view can
read.

Model-side only. Nothing drains these bits until [[0302]]; both shells keep
their existing poll/memo paths through this ticket, so it lands green and inert.

## Design

Per [[0300]]'s ADR:

- **`dirty_values`** — `DirtyBits<3>` from [[0299]] (181 ids → 3 words, tail
  masked). Flipped by [`set`](../../vxn-1b/crates/vxn1b-engine/src/shared.rs#L131)
  and [`set_normalized`](../../vxn-1b/crates/vxn1b-engine/src/shared.rs#L167)
  after the value store, `Release`. Seeded all-set so the first tick after open
  broadcasts the table.
- **`dirty_matrix`** — granularity per the ADR. Flipped by
  [`edit_matrix_slot`](../../vxn-1b/crates/vxn1b-engine/src/shared.rs#L271) and
  by the bulk paths.
- **`key_dirty` split** — the existing flag keeps its audio-thread semantics
  (`take_key_state`); a second view-side bit is added alongside, cleared only by
  the tick.

The three bulk writers must flip everything, and each is a place the current
design silently notifies nobody:

| writer | ref | today |
|---|---|---|
| `restore_from_bytes` | [shared.rs:339](../../vxn-1b/crates/vxn1b-engine/src/shared.rs#L339) | sets `reload` + `key_dirty`; **no per-param signal** |
| `copy_layer` | [shared.rs:240](../../vxn-1b/crates/vxn1b-engine/src/shared.rs#L240) | writes ~80 params via `self.set`, rewrites a matrix, flips key mode; **no view signal at all** |
| `edit_matrix_slot` | [shared.rs:271](../../vxn-1b/crates/vxn1b-engine/src/shared.rs#L271) | `reload` only |

`copy_layer` calling `self.set` per param means it gets value bits for free once
`set` marks — which is the point of doing this at the model rather than the
shell.

## Acceptance criteria

- [ ] `SharedParams` carries value + matrix + view-key dirty channels, all
      drained by a single documented reader.
- [ ] `set` / `set_normalized` mark; a test asserts a write then a drain yields
      exactly that id, and a second drain yields nothing.
- [ ] Seeded-full construction: the first drain on a fresh `SharedParams` yields
      all 181 ids.
- [ ] `restore_from_bytes` marks the full value table, the matrix and the key
      channel — one test per channel.
- [ ] `copy_layer` marks the copied params, the target layer's matrix and the key
      channel; the mixer-strip params it deliberately excludes
      (`COPY_LAYER_EXCLUDED`) are **not** marked.
- [ ] `take_reload` / `take_key_state` semantics are unchanged — the audio
      thread's re-sync path behaves exactly as before, pinned by the existing
      tests.
- [ ] `cargo test -p vxn1b-engine` green; `cargo test --workspace` green (both
      shells still on their old paths, so nothing else should move).

## Notes

- Additive only: one `fetch_or(Release)` per param write. ADR 0003's perf note
  applies — this is not a measurable cost, and it lands on the audio thread's
  publish path, so keep it a single atomic and don't grow it into a lock.
- No shell changes here. Resist wiring a drain "while you're in there" — the
  bisect value of a separate engine commit is the whole point ([[E046]] risks).
- One `cargo test` at a time — [[vxn-no-parallel-cargo-test]]. No `cargo fmt` —
  [[vxn-no-cargo-fmt]]. Stage explicit paths —
  [[vxn-concurrent-vxn2-work-no-git-add-all]].
- Blocks 0302 and 0303.
