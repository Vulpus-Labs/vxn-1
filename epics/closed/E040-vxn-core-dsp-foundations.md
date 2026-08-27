---
id: E040
product: monorepo
title: "vxn-core-dsp foundations — shared component crate, measurement harness, leaf + pure-move kernels (bit-exact)"
status: closed
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

## Codegen + perf baselines (captured by 0223, 2026-08-27)

Reference numbers every later extraction is checked against. Machine: macOS 14 /
aarch64, rustc 1.95.0 pinned, `[profile.release]` (thin LTO, opt-level 3,
codegen-units 1). Captured from the **linked cdylibs**, not per-crate objects —
thin LTO means a crate-level `.o` does not show what ships.

Re-check with `cargo run -p vxn-asm-check --release`. It exits non-zero if a
watched symbol falls below its floor or vanishes entirely.

### SIMD instruction counts per hot symbol

| symbol | artefact | captured | floor |
|---|---|---:|---:|
| `bank::RenderBank::render` | vxn1b | 9632 | 6000 |
| `poly::oscillator::PolyOscillator::process` | vxn1b | 292 | 200 |
| `engine::Engine::process_block` | vxn1b | 282 | 180 |
| `fx::FxChain::process_block` | vxn1b | 133 | 80 |
| `voice::Voices::fill_stack_pos` | vxn1b | 108 | 60 |
| `engine::Engine::cook_stacks_block` | vxn2 | 245 | 140 |
| `stack::lane_route_algo_*` (summed) | vxn2 | 286 | 200 |
| `stack::Stack::note_on` | vxn2 | 142 | 50 |
| `stack::stack_tick_stereo` | vxn2 | 196 | 35 |

Floors are **not** equalities. Instruction selection drifts a few percent with
any unrelated edit; what matters is the cliff — a de-vectorised kernel drops to
zero or near it, never by 5%. Floors sit at roughly 60-70% of capture.

`lane_route_algo_*` is summed across the 22 symbols that survive linking; the
source defines 32, and the linker folds identical ones. Watch the sum, not the
count, or ICF changes read as regressions.

### Throughput

| harness | captured |
|---|---|
| `vxn-engine/examples/busy_profile` (16 voices, Dual, 4× OS, sync + PM, FX on) | 320 s audio in 9.48 s = **33.8× realtime** |

### Criterion (`vxn2-osc-bench`, median of 100 samples)

Captured at reduced sample time (`--warm-up-time 1 --measurement-time 3`) — the
absolute numbers are comparable run-to-run on this machine, which is what a
before/after needs; they are not comparable to a default-settings run.

| bench | median |
|---|---:|
| `stack/stack_d1` | 54.57 µs |
| `stack/stack_d4` | 54.82 µs |
| `stack/stack_d8` | 55.08 µs |
| `filter_path/filter_off` | 401.9 µs |
| `filter_path/filter_on_4x` | 1.321 ms |
| `filter_quiescence/sustaining_4x` | 1.317 ms |
| `filter_quiescence/released_rungout_4x` | 16.07 µs |
| `master_chain/master_chain_full` | 399.6 µs |
| `master_chain/master_chain_fx_off` | 405.2 µs |

Note `stack_d1` → `stack_d8` costs +0.9% for 8× the density — the SoA lane
vectorisation working as designed ([[vxn2-stack-soa]]). If that spread ever
opens up, the lane loop went scalar and asm-check should have caught it first.

Not captured: `op_voice`, `voice`, `reverb`, `alloc`, `lfo`, `matrix`,
`matrix_gated`, `delay`, `cleanup`. A full sweep is tens of minutes and
[[vxn-no-parallel-cargo-test]] forbids overlapping it with anything else. Extend
the table when a ticket needs a specific bench as its before/after.

## Acceptance

- Full workspace test suite green (serialised runs) with zero golden changes.
- asm-check NEON-operand counts per named hot symbol unchanged vs the 0223
  reference; criterion deltas within noise.
- `vxn-core-dsp` builds standalone and via all three synth cdylibs.

## Close-out (2026-08-27)

All six tickets closed. `crates/vxn-core-dsp` exists and carries the component
layer; `crates/vxn-asm-check` makes "vectorisation unchanged" a checkable claim
rather than a comment.

**Every product bit-exact throughout.** vxn-2 render hash `0x533a37a7def1921a`
at every step; vxn-1 `baseline_render_is_stable` and its 7 declick tests
unmodified; workspace green at each close (1638 → 1641 → 1672 → 1670, the dip
being duplicated test helpers deleted).

**Three of the epic's premises turned out to be stale**, all true when written
on 2026-08-02 and drifted since. Recorded because the pattern matters more than
the instances: a ticket that says "these are already duplicates" is asserting
something about a *moving* tree, and by the time it is worked the claim may have
expired.

- **0224/0225** — `ms_to_samples` was not one function: truncating with floor 0
  in core-utils, rounding with floor 1 in vxn-1. Same name, different contract,
  diverging at 44.1 kHz. Kept both, added `fade_len_samples`.
- **0226** — `CONTROL_BLOCK` had three definitions, not two, one of them across
  a wire boundary held together by a comment.
- **0227** — `DynamicsBlock` had diverged eight days earlier (0241's metering
  tap), and the two OTA ladders differ in *voicing*: vxn-2 caps resonance at
  high cutoff, vxn-1/vxn-1b do not. That one could not be unified at all; the
  mechanism is shared and the policy stays per-synth.

**Two tickets ran ahead of their own measurement tool.** 0224 and 0225 landed
before 0223's asm-check existed and claimed "vectorisation unchanged" on
reasoning. 0226 and 0227 got the real thing: every watched symbol byte-for-byte
identical to the 0223 capture, `RenderBank::render` included at 9632 — the
symbol the ladder inlines into, and the one that would have moved had the
extraction cost anything.

**Deferred to E041 by design:** `DynamicsBlock`'s `WetFade` adoption, which
0227 sanctions deferring. `WetFade` exists and is tested; adopting it is an FX
change, not a move.
