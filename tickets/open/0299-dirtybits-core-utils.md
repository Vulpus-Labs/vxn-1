---
id: "0299"
product: monorepo
title: "DirtyBits in vxn-core-utils: one bitset primitive for three synths"
priority: medium
created: 2026-08-25
epic: E046
depends: []
---

## Summary

First ticket of [E046](../../epics/open/E046-dirty-bitset-pump-vxn1-vxn1b.md).
vxn-2's dirty-bitset pump is a good primitive wearing a vxn-2-shaped coat: the
words, the seeding, the `fetch_or(Release)` / `swap(Acquire)` pair and the
set-bit walk all live inline in
[`vxn2-engine::shared`](../../vxn-2/crates/vxn2-engine/src/shared.rs#L310-L595)
and in [`drain_dirty_bits`](../../vxn-2/crates/vxn2-web-controller/src/lib.rs#L38).
E046 adds two more consumers (three, counting vxn-1's second model impl), so
hoist it before copying it.

Extract to `crates/vxn-core-utils` and adopt in `vxn2-engine` with **no
behaviour change** — a REBASELINE commit, per the [[vxn-core-dsp-extraction]]
idiom. vxn-2 is the proving ground precisely because it is already shipping this
code: if the hoisted type changes anything, vxn-2's suite says so.

## Design

Minimum viable surface, from what the three consumers actually need:

- `DirtyBits<const N_WORDS: usize>` over `[AtomicU64; N_WORDS]`, constructed
  either empty or **fully seeded** — vxn-2 seeds all-set so the first tick after
  open broadcasts the whole table
  ([shared.rs:434](../../vxn-2/crates/vxn2-engine/src/shared.rs#L434)), and both
  new consumers want the same.
- `mark(id)` — `fetch_or(Release)`.
- `mark_all()` — the bulk-store path
  ([`mark_all_dirty`](../../vxn-2/crates/vxn2-engine/src/shared.rs#L553)).
- `drain(|id| …)` or `take() -> [u64; N_WORDS]` — `swap(0, Acquire)` per word
  then the `trailing_zeros` / `bits &= bits - 1` walk. Prefer the callback form:
  it keeps the walk in one place instead of re-appearing in every drain site,
  and it lets the caller skip vxn-2's per-tick `Vec` allocation (finding 7 of
  [0298](0298-vxn2-web-controller-smells.md)).
- Out-of-range masking: vxn-2's
  [`dirty_values_full_word`](../../vxn-2/crates/vxn2-engine/src/shared.rs#L314)
  keeps the tail word's unused bits clear so `mark_all` + drain never emit a
  phantom id. Generalise it — the param counts differ per synth (209 / 181 / 165).

The single-bit flags (`dirty_ks_curve`, VXN1b's key channel) stay plain
`AtomicBool`; a one-bit `DirtyBits` buys nothing.

Ordering is the contract and belongs in the type's doc comment, not in each
caller: writer stores value `Relaxed` **then** `mark` `Release`; the sole reader
drains `Acquire` then loads `Relaxed`. Copy the race-window analysis from
[ADR 0003 § Correctness](../../vxn-2/adrs/0003-dirty-bitset-diff-pump.md) — it is
the part most likely to be re-derived wrongly by a future consumer.

`vxn-core-utils` is `no_std`-ish DSP-adjacent today (ftz, smoothing, meter,
scope); a lock-free change-channel is a reasonable neighbour and is already
where `MeterBus` lives ([[vxn-metering-spine]]). If it reads wrong there, the
alternative is `vxn-core-app` — but that would make `vxn2-engine` depend on the
controller crate, which it currently does not.

## Acceptance criteria

- [ ] `DirtyBits` lives in `vxn-core-utils` with the ordering contract and the
      race-window argument in its doc comment.
- [ ] `vxn2-engine`'s `dirty_values` / `dirty_matrix` are `DirtyBits`; the
      inline word-masking and drain-walk helpers are gone, not merely unused.
- [ ] `vxn2-web-controller::drain_dirty_bits` uses the callback drain and no
      longer allocates a `Vec<bool>` + `Vec<ViewEvent>` per tick.
- [ ] Unit tests on the primitive: seeded-full drains every valid id once;
      tail-word bits beyond the count never surface; a second drain after a
      drain with no writes yields nothing; `mark` after a drain surfaces next
      drain.
- [ ] **No behaviour change in vxn-2** — `cargo test -p vxn2-engine`,
      `-p vxn2-web-controller`, `-p vxn2-clap` and the vxn-2 web node suite all
      green with no test edits beyond mechanical renames.
- [ ] Commit message marks it REBASELINE (extraction, not a fix).

## Notes

- Do **not** change vxn-2's semantics here — findings 1, 2 and 7 of
  [[0298]] are that ticket's call, except 7, which this one fixes as a
  side effect of the callback drain. Say so in 0298 when it lands.
- One `cargo test` at a time — [[vxn-no-parallel-cargo-test]]. No `cargo fmt` —
  [[vxn-no-cargo-fmt]]. Stage explicit paths —
  [[vxn-concurrent-vxn2-work-no-git-add-all]].
- Blocks 0301 and 0304.
