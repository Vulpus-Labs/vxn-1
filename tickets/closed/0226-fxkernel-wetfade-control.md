---
id: "0226"
product: monorepo
title: "vxn-core-dsp vocabulary modules: control (CONTROL_BLOCK, rate newtypes, UpdateRate), declick (WetFade), fx (FxKernel), test_util"
priority: high
created: 2026-08-02
epic: E040
depends: ["0222"]
---

## Summary

Fifth ticket of [E040](../../epics/open/E040-vxn-core-dsp-foundations.md). The
contract layer everything later builds on.

- **control**: single `CONTROL_BLOCK = 32` definition —
  [vxn-dsp/src/lib.rs:57](../../vxn-1/crates/vxn-dsp/src/lib.rs#L57) becomes a
  re-export, [vxn2-clap/src/lib.rs:56](../../vxn-2/crates/vxn2-clap/src/lib.rs#L56)
  imports it (values identical, zero behaviour). `UpdateRate { Snap, Block,
  Quantum(u32), PerSample }` naming taxonomy. Rate newtypes `BaseRate` /
  `OsRate` / `CtrlRate` (guards two live hazards: `OtaLadderCoeffs::new` takes
  the **oversampled** rate for its fs-dependent pole detune while `k_cap` is
  absolute Hz; vxn-1's `LfoCore` is constructed at the **control** rate,
  [voice.rs:501](../../vxn-1/crates/vxn-engine/src/voice.rs#L501)).
- **declick**: `WetFade` — the vxn-2 enable idiom extracted from
  [dynamics.rs](../../vxn-2/crates/vxn2-dsp/src/dynamics.rs) (`enabled` +
  `mix: Smoothed` + `mix_primed`): `set_enabled(on) -> EdgeAction{None,
  RisingClear}`, `set_mix`, `tick()`, `settled_off()`, `snap()`. Caller clears
  audio state on `RisingClear`; `settled_off()` licenses bit-exact passthrough.
- **fx**: `trait FxKernel { type Params; new(sr); set_params(&P);
  process(l, r) -> (f32, f32); process_block (default loops process;
  overrides must be sample-identical — tested); reset(); clear();
  is_active(); state_abs_max() }`. Used monomorphically only — the trait
  exists for contract uniformity + test reuse, never dyn dispatch.
- **test_util**: move `assert_bit_exact_passthrough` /
  `assert_bit_exact_after_settle` (canonical:
  [vxn2-dsp/src/test_util.rs](../../vxn-2/crates/vxn2-dsp/src/test_util.rs);
  delete the inlined copy in
  [vxn-dsp/src/dynamics.rs tests](../../vxn-1/crates/vxn-dsp/src/dynamics.rs))
  plus a shared `worst_d4`/`join_d4` (from vxn-1's declick suite).

## Acceptance criteria

- [ ] All four modules compile with unit tests (WetFade edge/settle semantics,
      FxKernel default process_block equivalence harness).
- [ ] `CONTROL_BLOCK` greps to exactly one literal definition repo-wide.
- [ ] No synth behaviour change: all hashes/parity byte-identical.

## Notes

`state_abs_max()` default: `0.0` when `!is_active()`, else `f32::INFINITY` —
tailed kernels (reverb, resonant filters) override; vxn-2's span quiescence
gate (`OtaLadderKernel::state_abs_max`) is the model.

## Close-out (2026-08-27)

All four modules land in [`vxn-core-dsp`](../../crates/vxn-core-dsp/src/), 25
unit tests, `cargo test -p vxn-core-dsp` green.

### control

- **`CONTROL_BLOCK` now greps to exactly one literal definition repo-wide.**
  There were **three**, not the two the ticket names —
  [vxn2-wasm/src/lib.rs:31](../../vxn-2/crates/vxn2-wasm/src/lib.rs#L31) had its
  own, carrying the comment *"Must equal `vxn2-clap`'s `CONTROL_BLOCK` (32) —
  the block-rate cadence is part of the sound."* A hand-maintained invariant
  across a wire boundary, with nothing enforcing it. All three now re-export.
- A **fourth**, differently-named mirror: `const CB: usize = 32; //
  CONTROL_BLOCK` in
  [vxn1b-engine/tests/zipper_regression.rs](../../vxn-1b/crates/vxn1b-engine/tests/zipper_regression.rs#L21).
  It satisfies the criterion as literally worded (it is not spelled
  `CONTROL_BLOCK`) but is the same hazard — a test asserting a per-block cadence
  against its own copy of the block size would silently stop testing that if the
  two diverged. Repointed at `vxn_dsp::CONTROL_BLOCK`.
- `UpdateRate { Snap, Block, Quantum(n), PerSample }` + `stride()`. Vocabulary
  only; *which* param sits in which class stays per-synth, per ADR 0002 §6.
- `BaseRate` / `OsRate` / `CtrlRate` newtypes. These turn the ticket's two named
  hazards into type errors instead of comments: `OtaLadderCoeffs::new` wants the
  **oversampled** rate while its `k_cap` is absolute Hz, and vxn-1's `LfoCore` is
  built at the **control** rate. Both currently "work" if handed the wrong f32 —
  they just mistune, which is the worst failure mode available.

### declick

`WetFade` — `enabled` + smoothed `mix` + first-set snap + edge flag, generalised
out of `DynamicsBlock` where every vxn-2 effect had reimplemented it.

**The edge is reported from `tick()`, not `set_enabled()`**, which departs from
the ticket's `set_enabled(on) -> EdgeAction` sketch. It has to: after
`set_enabled(true)` the effect may *already* be active because a switch-off fade
never completed, and there is no stale state to clear in that case — clearing
would cut the live tail. `a_toggle_that_never_completes_its_fade_reports_no_edge`
pins it. This matches what vxn-2 actually does (`was_active`, checked in the
audio path), which is the behaviour 0227/E041 have to preserve.

`settled_off()` returns exactly `+0.0` and stops ticking the smoother, so the
caller's passthrough can be bit-exact rather than merely inaudible.

### fx

`FxKernel` with a default `process_block` that loops `process`, plus
`Bypassable<K>` carrying the off→on edge-reset glue that vxn-1 spells as
`limiter_fade` + `limiter_was_on`. Monomorphic use only — ADR 0002 §4 forbids
dyn in a sample loop; the trait exists for contract uniformity and test reuse.
`state_abs_max()` defaults to `0.0` when inactive, `INFINITY` otherwise, per the
ticket's Notes.

### test_util

Canonical homes for `assert_bit_exact_passthrough` /
`assert_bit_exact_after_settle` / `worst_d4` / `join_d4`, plus a new
`assert_block_matches_sample` — the `process_block`-equivalence harness the
ticket asks for, written once here rather than in each of 0228–0232.

Deleted: 25 lines of hand-copied helpers from
[vxn-dsp/src/dynamics.rs](../../vxn-1/crates/vxn-dsp/src/dynamics.rs) (whose own
comment admitted the copy), 41 from
[vxn2-dsp/src/test_util.rs](../../vxn-2/crates/vxn2-dsp/src/test_util.rs), and
two byte-identical `worst_d4` copies in vxn-1's and vxn-2's declick harnesses.
Both crates keep re-export shims so in-crate paths still resolve.

**One overclaim of mine, corrected before it shipped.** I documented
`assert_bit_exact_passthrough` as catching an `x * 1.0 + 0.0` implementation. It
does not — the two agree bitwise on every value in the helper's fixed sine
input, diverging only at `-0.0`, which those inputs never produce. My own
`should_panic` test failed and was right to. The doc now carries an explicit
"what it does not catch", and the test demonstrates the blind spot directly
instead of pretending it away. The helper still catches the realistic failures
(residual gain, a wet path that never quite reached zero). That gap is also the
argument for `settled_off()` licensing the caller to skip the arithmetic
entirely rather than blessing a cheap-looking equivalent.

### Verification

- Workspace **1672 passed / 0 failed**, 104 suites.
- vxn-2 render hash **`0x533a37a7def1921a`** — unchanged.
- **asm-check: every watched symbol byte-for-byte identical to the 0223
  capture** — 9632 / 292 / 282 / 133 / 108 / 245 / 286 / 142 / 196, not merely
  above floor. This is the first E040 ticket to get the measured guarantee
  rather than a reasoned one; 0224 and 0225 landed before the tool existed.
