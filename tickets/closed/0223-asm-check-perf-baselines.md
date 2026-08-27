---
id: "0223"
product: monorepo
title: "asm-check xtask harness + criterion/busy_profile baseline capture"
priority: high
created: 2026-08-02
epic: E040
depends: ["0222"]
---

## Summary

Second ticket of [E040](../../epics/open/E040-vxn-core-dsp-foundations.md).
Every later extraction claims "vectorisation unchanged" — make that checkable.
Add an xtask subcommand that disassembles release artefacts and counts NEON
vector operands per named hot symbol, and record the pre-extraction reference
counts + perf numbers.

## Design

- `llvm-objdump -d` (toolchain copy — host `nm`/tools broken on these
  staticlibs, see [[vxn-host-nm-broken-llvm22]]) over the release cdylibs /
  bench binaries.
- Symbols: vxn-1 poly oscillator kernels (`process_sync_w`/`process_pm_w`
  monomorphs), `poly/ladder` `process_w` monomorphs, vxn-2's 32
  `#[inline(never)]` lane-route symbols
  ([stack.rs:112-180](../../vxn-2/crates/vxn2-dsp/src/stack.rs#L112-L180)).
- Count instructions with NEON operands: grep `v[0-9]+\.4s` **on operand
  text, not mnemonic-anchored** — on ARM64 the `.4s` arrangement suffix sits
  on the mnemonic (`fadd.4s v0, v0, v2`), so a mnemonic-anchored pattern
  reports zero on vectorised code ([[vxn1-neon-grep-pitfall]]).
- Record: criterion baselines (`vxn2-osc-bench`: stack, filter_path, op_voice,
  master_chain, reverb, voice; vxn-1 benches) and
  `vxn-engine/examples/busy_profile.rs` output, noted in the epic file.

## Acceptance criteria

- [ ] `cargo xtask asm-check` (or equivalent) emits per-symbol NEON-operand
      counts and exits non-zero on a symbol dropping to zero.
- [ ] Reference counts + criterion/busy_profile numbers recorded in E040.
- [ ] Runs green on current main.

## Notes

Never run cargo test/bench concurrently in this repo
([[vxn-no-parallel-cargo-test]]). Per-crate asm can mislead pre-LTO —
disassemble the *linked* artefacts ([[vxn1-ota-filter-perf]]).

## Close-out (2026-08-27)

- [`crates/vxn-asm-check`](../../crates/vxn-asm-check/src/main.rs) — a bin crate,
  run as `cargo run -p vxn-asm-check --release`. Disassembles the **linked
  release cdylibs** with `llvm-objdump`, attributes each SIMD instruction to the
  symbol it falls under, and compares against recorded floors. Exit **0** when
  every watched symbol holds; exit **1** when one falls below its floor or
  vanishes entirely — both paths verified by running them.
- Not an `xtask` subcommand: the ticket allowed "or equivalent", and this watches
  symbols in **two products' artefacts**. Putting a cross-product tool inside
  one product's xtask would have meant either duplicating it or picking an
  arbitrary owner, and [[0317]] is going to consolidate the xtasks anyway.
- Reference counts + throughput recorded in
  [E040](../../epics/open/E040-vxn-core-dsp-foundations.md), with the machine and
  profile they were captured on.

### The ticket's counting rule was backwards, and would have made the tool useless

0223's Design says to grep `v[0-9]+\.4s` **"on operand text, not
mnemonic-anchored"**. Its *reasoning* is right — on ARM64 the arrangement suffix
sits on the mnemonic — but the *instruction* inverts it. Measured against
`libvxn1b_clap.dylib`:

| pattern | matches |
|---|---:|
| operand-anchored `v[0-9]+\.4s` (as the ticket specifies) | **5** |
| the true count | **8940** |

A harness written to the ticket's letter would have reported near-zero on fully
vectorised code and then "passed" every future extraction by finding nothing to
lose — the exact failure [[vxn1-neon-grep-pitfall]] exists to warn about,
reproduced inside the tool meant to catch it.

The rule used instead is **syntax-agnostic**: match the arrangement suffix
anywhere in the instruction text, so both `fadd.4s v0, v1` (Apple/LLVM) and
`fadd v0.4s, v1.4s` (canonical ARM) count. CI may run a different objdump than a
dev machine and the number must not depend on which.

**The first implementation got this wrong too**, taking `.nth(1)` after
splitting on tabs — which is the mnemonic field only, so canonical syntax was
missed. `both_aarch64_syntaxes_count_as_simd` caught it before it ever ran on a
real artefact. That test is the most load-bearing one in the crate; keep both
syntaxes in it.

### Watch set differs from the ticket's guesses

The ticket names `process_sync_w` / `process_pm_w` / `poly/ladder process_w`
monomorphs. **None exists as a distinct symbol** — they are plain `fn`s and
inline into their callers. The real hot symbols, discovered from the artefacts:
`RenderBank::render` (9632 SIMD, by far the largest), `PolyOscillator::process`,
`Engine::process_block`, `FxChain::process_block`, `Voices::fill_stack_pos`, and
on vxn-2 `cook_stacks_block`, `Stack::note_on`, `stack_tick_stereo` and the
`lane_route_algo_*` family.

The lane routes are watched as a **sum**, not per symbol: the source defines 32,
22 survive linking because the linker folds identical ones. Watching the count
would read ICF changes as regressions.

Floors are set at roughly 60-70% of capture, deliberately not equalities —
instruction selection drifts a few percent with unrelated edits, and the failure
this guards against is a cliff to zero, not a 5% slide.

### Deferred

- Criterion coverage is partial by design: all 9 groups of `stack`,
  `filter_path` and `master_chain` are captured, at reduced sample time
  (`--warm-up-time 1 --measurement-time 3`). `op_voice`, `voice`, `reverb`,
  `alloc`, `lfo`, `matrix`, `matrix_gated`, `delay` and `cleanup` are not — a
  full sweep is tens of minutes and [[vxn-no-parallel-cargo-test]] forbids
  overlapping it with anything else. The captured numbers are comparable
  run-to-run on this machine, which is what a before/after needs; they are not
  comparable to a default-settings run, and the epic says so.
- **0224 and 0225 landed before this tool existed** and each claimed
  "vectorisation unchanged" on reasoning alone. Both are pure moves of
  identical-valued constants and expressions, and the current asm-check run is
  green — but that is a *post-hoc* check against floors captured *after* they
  landed, not a before/after. Nothing suggests a regression; it is simply not
  measured. E040's remaining tickets (0226, 0227) get the real thing.
