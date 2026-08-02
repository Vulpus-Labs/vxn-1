---
id: E040
product: monorepo
title: "vxn-core-dsp foundations — shared component crate, measurement harness, leaf + pure-move kernels (bit-exact)"
status: open
created: 2026-08-02
---

> **Stand up the shared DSP component layer.** vxn-1's `vxn-dsp` and vxn-2's
> `vxn2-dsp` are parallel forks: `DynamicsBlock` is a documented verbatim port
> (48-line diff = import path + test helpers), `HpfKernel` is effectively
> identical, the scalar OTA ladder / phaser / reverb are forks with mechanical
> deltas. Root [ADR 0001](../../adrs/0001-vxn-core-split.md) §2 kept DSP
> synth-local but says "revisit if a third synth shows up" — vxn-1b + vxn-2 +
> vxn-3-as-FX-consumer meet that condition. This epic creates
> `crates/vxn-core-dsp`, the codegen measurement harness, and lands every
> extraction that is a **pure move** — zero behaviour change, all goldens
> byte-identical.

## Goal

When this epic closes:

- `crates/vxn-core-dsp` exists (deps: `vxn-core-utils` only), with the shared
  vocabulary modules: `control` (`CONTROL_BLOCK`, `UpdateRate`, rate newtypes
  `BaseRate`/`OsRate`/`CtrlRate`), `declick` (`WetFade` — the vxn-2 enable
  idiom extracted), `fx` (`FxKernel` trait), `test_util` (shared bit-exact +
  d4 helpers).
- ADR `adrs/0002-vxn-core-dsp.md` records the revised extraction boundary
  (supersedes ADR 0001 §2 for the component layer).
- `DynamicsBlock`, `HpfKernel`, and the scalar `OtaLadderKernel`/
  `OtaLadderCoeffs` live in `vxn-core-dsp`; `vxn-dsp`/`vxn2-dsp` keep their
  module paths as re-export shims (the established `vxn2-dsp/src/smoother.rs`
  pattern).
- `HalfbandInterp`/`Interpolator`, `BypassXfade`/`raised_cosine_rise` (open
  ticket 0195), and the triplicated Q32 phase constants live in
  `vxn-core-utils`.
- An xtask asm-check subcommand + recorded criterion/busy_profile baselines
  exist so every later epic can verify vectorisation didn't regress.

## Constraints

- **Bit-exact throughout**: vxn-1 baseline, vxn-1b parity oracle, vxn-2 render
  hash, declick d4 suite all pass unmodified in every commit of this epic.
- Nothing SoA-hot moves: `poly/oscillator.rs`, `poly/ladder.rs` bodies,
  `stack.rs` stay put; only consts/coeff types they import move.
- Shared code: plain `#[inline]`, no dyn, no enum-match in sample loops.
- Toolchain stays pinned 1.95.0 (codegen-sensitive goldens).

## Planned tickets

Dependency chain: **0222 → {0223, 0224, 0225, 0226} → 0227**.

- [ ] **0222** — ADR 0002-vxn-core-dsp + crate scaffold.
- [ ] **0223** — asm-check xtask harness + criterion/busy_profile baseline capture.
- [ ] **0224** — Leaf moves: `HalfbandInterp`/`Interpolator` + Q32 consts → vxn-core-utils.
- [ ] **0225** — `BypassXfade` + `raised_cosine_rise` → vxn-core-utils (absorbs ticket 0195).
- [ ] **0226** — `control` / `declick` / `fx` / `test_util` modules.
- [ ] **0227** — Pure-move kernels: DynamicsBlock, HpfKernel, scalar OTA ladder.

## Acceptance

- Full workspace test suite green (serialised runs) with zero golden changes.
- asm-check NEON-operand counts per named hot symbol unchanged vs the 0223
  reference; criterion deltas within noise.
- `vxn-core-dsp` builds standalone and via all three synth cdylibs.
