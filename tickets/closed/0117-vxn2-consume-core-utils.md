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

## Close-out (2026-06-26)

- **Smoothing.** `vxn-core-utils::smoothing::Smoothed` now derives
  `Copy + Debug` ([smoothing.rs:31](../../crates/vxn-core-utils/src/smoothing.rs#L31)).
  `vxn2-dsp` and `vxn2-engine` gained `vxn-core-utils = { workspace = true }`.
  `vxn2-dsp/src/smoother.rs` is now a re-export shim of
  `Smoothed`/`one_pole_coeff`/`ms_to_samples` — the forked definitions and their
  duplicate tests are gone; all `crate::smoother::…` / `vxn2_dsp::smoother::…`
  call sites (matrix, master, dynamics, reverb, delay, phaser, limiter, engine)
  resolve to the shared copy.
- **Flush-to-zero.** `vxn2-engine/src/ftz.rs` deleted; `lib.rs` re-exports
  `vxn_core_utils::ScopedFlushToZero` (core's raw-`ldmxcsr` x86 path, picked per
  ticket). The third inline `flush_denormal` copy in `phaser.rs` replaced by
  `vxn_core_utils::ftz::flush_denormal`. NOTE: vxn-2's inline copy used a
  `|x| < 1e-30` gate vs core's `is_normal`-based guard — the ticket's "same
  semantics" was inexact. The divergent band (1.18e-38..1e-30) is unreachable by
  the phaser `fb_state` (signal-level or exactly 0, bounded by
  `fast_tanh(fb·fb_state)`); both zero sub-(-600 dB) values, so the swap is
  inaudible/behaviour-preserving. All property tests (zipper/click/audibility)
  green.
- **MIDI pitch.** `vxn2-dsp::op::midi_to_hz` deleted; callers use
  `vxn_core_utils::note_to_hz(key as f32)` ([op.rs:124](../../vxn-2/crates/vxn2-dsp/src/op.rs#L124)).
  Core's `note_to_hz` re-anchored to `440·2^((n−69)/12)` with `powf` (not
  `exp2`) to stay **bit-identical** to vxn-2's shipped formula — verified 7-ulp
  divergence between the old forms, so an exact match was required to preserve
  the render. Safe: core `note_to_hz` had zero production consumers (vxn-1's
  audio path uses its own `fast_exp2` variant in `vxn-dsp`, left untouched as a
  deliberate fast variant). vxn-2 only ever calls with integer notes, so
  fractional rounding is moot.
- **Tempo-sync.** `vxn2-dsp/src/lfo.rs` `SUBDIVISIONS`/`Subdivision`/
  `index_from_norm`/`synced_hz`/`s`/`T` replaced by a re-export of
  `vxn-core-utils::sync` (`subdivision_hz as synced_hz`). `vxn-app/src/sync.rs`
  likewise delegates its table + `index_from_norm` + `synced_hz`/`synced_seconds`
  to core (aliased to the `synced_*` names), keeping its per-synth CLAP-id
  helpers (`sync_partner_clap_id` etc.). Table values + `(bpm/60)/beats` math are
  identical, so vxn-1 render is unchanged.
- **xorshift cross-ref.** Added a doc note on vxn-1's `xorshift64`
  ([math.rs:6](../../vxn-1/crates/vxn-dsp/src/math.rs#L6)) flagging it as a
  deliberately different generator from vxn-2's `xorshift64*` (`rng.rs`) — not to
  be merged (different output streams).
- **Single-definition sweep.** `grep -rn` across `crates/`, `vxn-1/crates/`,
  `vxn-2/crates/` finds exactly one definition of `struct Smoothed`,
  `one_pole_coeff`, `ms_to_samples`, `struct ScopedFlushToZero`,
  `flush_denormal`, `static SUBDIVISIONS`, `struct Subdivision`,
  `index_from_norm` (all in `vxn-core-utils`); `fn midi_to_hz` now has zero
  definitions (folded into `note_to_hz`).
- **Tests.** `cargo test --workspace` green (0 failed). Fixed a pre-existing,
  unrelated break in `vxn2-clap` `editor_smoke::load_factory_round_trips_into_shared_params`
  (Bass-category presets had been added, shifting factory index 0 off
  Brass/Analog Brass) — the test now locates Analog Brass by name rather than a
  fixed index.
