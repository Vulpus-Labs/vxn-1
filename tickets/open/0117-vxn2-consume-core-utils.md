---
id: "0117"
product: vxn-2
title: vxn-2 consumes vxn-core-utils — delete forked smoothing/ftz/midi/sync
priority: high
created: 2026-06-23
epic: E027
---

## Summary

`vxn2-dsp/Cargo.toml` has an empty `[dependencies]` and
`vxn2-engine` pulls `vxn-core-app` but never `vxn-core-utils`.
So vxn-2 carries hand-maintained copies of primitives the
shared crate already owns — each a "fix both or drift" trap.
None are SIMD hot-loop code; all are safe to share.
Behaviour-preserving.

Forks to delete (canonical home in parentheses):

1. **Smoothing** — `Smoothed`, `one_pole_coeff`,
   `ms_to_samples`, `SNAP_EPS` in
   `vxn2-dsp/src/smoother.rs` (comment already admits
   "Lifted from VXN1"). Identical to
   `vxn-core-utils/src/smoothing.rs` except vxn-2 derives
   `Copy + Debug`.
2. **Flush-to-zero** — `ScopedFlushToZero` in
   `vxn2-engine/src/ftz.rs`, plus a third inline
   `flush_denormal` copy in `vxn2-dsp/src/phaser.rs:53`.
   Same semantics as `vxn-core-utils/src/ftz.rs` (x86 path
   diverged: vxn-2 uses `_MM_SET_FLUSH_ZERO_MODE`, core uses
   raw `ldmxcsr` — pick core's).
3. **MIDI pitch** — `midi_to_hz` at `vxn2-dsp/src/op.rs:107`
   vs `vxn-core-utils/src/midi.rs`.
4. **Tempo-sync** — `SUBDIVISIONS` / `Subdivision` /
   `index_from_norm` in `vxn2-dsp/src/lfo.rs:159` (comment
   pleads "intentional duplication … before divergence
   stabilises" — that rationale predates the extraction).
   The vxn-1 `vxn-app/src/sync.rs:25` copy is a third
   instance; fold it too (keep its per-synth helpers like
   `sync_partner_clap_id`).

## Acceptance criteria

- [ ] `vxn-core-utils::smoothing::Smoothed` gains `Copy +
      Debug`; `vxn2-dsp` and `vxn2-engine` add
      `vxn-core-utils = { workspace = true }`.
- [ ] `smoother.rs`, `ftz.rs`, the `phaser.rs` inline
      `flush_denormal`, the `op.rs` `midi_to_hz`, and the
      `lfo.rs` subdivision table are deleted and replaced by
      `vxn-core-utils` imports; `vxn-app/src/sync.rs`
      delegates its table to `vxn-core-utils::sync`.
- [ ] First-add a one-line cross-ref comment on vxn-1's
      `xorshift64` (`vxn-dsp/src/math.rs:10`) noting it is
      intentionally a different variant from vxn-2's
      `xorshift64*` (`vxn2-dsp/src/rng.rs:12`) — do **not**
      merge the RNGs (different output mappings).
- [ ] `grep -rn` across `crates/`, `vxn-1/crates/`,
      `vxn-2/crates/` finds exactly one definition of each
      deduped symbol.
- [ ] `cargo test --workspace` green; `vxn2-engine`
      `tests/baseline.rs` render hash unchanged.

## Notes

x86 FTZ paths differ in code but not semantics — keep core's
raw-asm version and confirm an x86 build still flushes
denormals (the ARM path is the production target). The
scalar `fast_tanh`, limiter, and half-band are **not** in
this ticket — they need promotion *into* core first
(`0118`). Mechanical, low risk; no SIMD hot-path code moves.
