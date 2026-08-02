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
