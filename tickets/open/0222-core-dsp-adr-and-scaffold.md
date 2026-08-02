---
id: "0222"
product: monorepo
title: "ADR 0002-vxn-core-dsp + crates/vxn-core-dsp scaffold"
priority: high
created: 2026-08-02
epic: E040
depends: []
---

## Summary

First ticket of [E040](../../epics/open/E040-vxn-core-dsp-foundations.md).
Record the revised extraction boundary and create the empty shared component
crate. Root [ADR 0001 §2](../../adrs/0001-vxn-core-split.md) deliberately kept
DSP primitives synth-local with the condition "revisit if a third synth shows
up" — met. The new ADR supersedes §2 **for the component layer only**: leaf
utils stay in `vxn-core-utils`; components (anything with a `Params` struct, a
sample-rate constructor, or an enable/declick lifecycle) go to
`crates/vxn-core-dsp`; hot SoA voice kernels stay per-synth.

## Design

- ADR `adrs/0002-vxn-core-dsp.md`: boundary rule above; the locked decisions
  (unify declick on the vxn-2 idiom, re-baselining allowed in flagged commits,
  runtime-swap design-for); the SIMD-protection regime (plain `#[inline]`, no
  dyn/enum-match in sample loops, fat-LTO erases crate boundary — verified by
  asm-check, not assumed); the not-extracted list (tick_ops, cook order,
  allocators, mod routing, smoothing policies, OsSpan FSM, vxn-3 send delay).
- `crates/vxn-core-dsp`: `[lib]` rlib, deps `vxn-core-utils` only. Empty
  module stubs: `control`, `declick`, `fx`, `env`, `os_region`, `test_util`.
  Workspace `members` + `[workspace.dependencies]` entries.

## Acceptance criteria

- [ ] ADR 0002 committed, cross-referenced from ADR 0001 (one-line addendum).
- [ ] `cargo build -p vxn-core-dsp` green; workspace builds unchanged.
- [ ] No synth crate depends on `vxn-core-dsp` yet (pure addition).

## Notes

Boundary test (goes in the ADR): every shared trait must be implementable by
all consumers without a fake parameter, a per-block `Box<dyn>`, or a
signal-model compromise. Related: [[vxn1-architecture]], [[vxn2-audio-kernel]].
