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
  phantom id. Generalise it — the param counts differ per synth (209 / 185 / 165).

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

## Close-out (2026-08-27)

- `DirtyBits<N_WORDS, N_BITS>` lives in
  [vxn-core-utils/src/dirty.rs](../../crates/vxn-core-utils/src/dirty.rs), with
  `words_for()` for the sizing. Surface as designed: `new_empty` / `new_all_set`,
  `mark`, `mark_all`, `drain(|id| …)`, `take()`, plus advisory `any` / `count`.
  Two const params rather than one derived from the other because
  `[AtomicU64; words_for(N)]` needs `generic_const_exprs`; the constructors
  debug-assert they agree.
- The ordering contract and **both** race-window arguments are in the module
  doc, transcribed from vxn-2 ADR 0003 § Correctness — writer stores `Relaxed`
  then marks `Release`; the sole reader drains `Acquire` then loads `Relaxed`;
  write-between-swap-and-load and write-between-store-and-mark are each shown to
  lose no event. The single-reader requirement is stated as a requirement, since
  `drain` is a `swap` and two drainers would split the change set.
- `mark` ignores an out-of-range id rather than panicking — it runs on the audio
  thread, where a dropped notification beats a panic.
- **Ten unit tests** on the primitive, sized at `N = 209` (vxn-2's
  `TOTAL_PARAMS`: three full words plus 17 bits, so the tail word is genuinely
  partial): seeded-full drains every valid id exactly once; tail-word padding
  never surfaces via *either* route (`new_all_set` and `mark_all`); a second
  drain with no writes yields nothing; `mark` after a drain surfaces next drain;
  repeated marks coalesce to one; out-of-range mark ignored; `take` returns the
  raw words and clears; an exact multiple of 64 has no padding. Plus a doctest
  on `words_for`.
- `vxn2-engine`: `dirty_values` and `dirty_matrix` are now `DirtyValues` /
  `DirtyMatrix` aliases of `DirtyBits`
  ([shared.rs:312-315](../../vxn-2/crates/vxn2-engine/src/shared.rs#L312-L315)).
  The inline helpers are **gone, not merely unused** — `dirty_values_full_word`
  and `DIRTY_MATRIX_ALL` no longer exist in the crate; grep confirms.
- `vxn2-web-controller::drain_dirty_bits` takes a callback and packs straight
  into the out-buffer. Both per-tick allocations are gone: the `Vec<ViewEvent>`
  and the `vec![false; TOTAL_PARAMS]`, the latter now a stack `[bool; 209]`.
  That is **finding 7 of [[0298]]**, fixed here as a side effect of the callback
  drain — 0298 should record it as already done and rule only on 1–6.
- **Scope note:** the sweep found a *second* hand-rolled copy of the set-bit walk
  in [vxn2-clap/src/lib.rs:219](../../vxn-2/crates/vxn2-clap/src/lib.rs#L219) —
  the native shell's `drain_dirty_bits`. Since "keep the walk in one place
  instead of re-appearing at every drain site" is this ticket's stated rationale,
  its walk was converted too. Its `-> Vec<ViewEvent>` signature was deliberately
  **kept**: ~20 call sites (mostly tests) consume the Vec, and changing it would
  have meant test edits well past "mechanical rename". Only the web controller
  was specified to lose its per-tick allocation, and only it did.
- The `id >= TOTAL_PARAMS` guard both walks carried is now the primitive's
  invariant, pinned by `tail_word_padding_never_surfaces` rather than re-checked
  per drain site.
- **No behaviour change in vxn-2**, no test edits beyond two mechanical renames:
  the removed private consts became a locally-computed tail mask and a
  test-local `DIRTY_MATRIX_ALL_TEST`.
