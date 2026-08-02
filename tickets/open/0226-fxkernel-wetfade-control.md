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
