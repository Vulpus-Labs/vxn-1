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

## Close-out (2026-08-27)

- [ADR 0002 — vxn-core-dsp](../../adrs/0002-vxn-core-dsp.md) committed. Records
  the three-layer boundary as a **mechanical** test (a `Params` struct, a
  sample-rate constructor, or a declick lifecycle → component layer), the
  no-fake-parameter / no-per-block-`Box<dyn>` / no-signal-model-compromise
  boundary test from ADR 0001 §6, the SIMD-protection regime (§4), the four
  locked E040 decisions (§5: declick on the vxn-2 idiom, REBASELINE-flagged
  commits, design-for-swap, delays on a vxn-2 superset), and the not-extracted
  list (§6: `tick_ops`/cook order, allocators, mod routing, smoothing policies,
  the `OsSpan` FSM, vxn-3's send delay).
- §4 corrects an assumption worth flagging: `[profile.release]` sets **thin**
  LTO, not fat, so "the crate boundary disappears" is a claim 0223's asm-check
  has to verify rather than one the ADR can assert.
- ADR 0001 §2 carries a one-line addendum pointing at ADR 0002 and scoping the
  supersession to the component layer
  ([0001:81-89](../../adrs/0001-vxn-core-split.md#L81-L89)).
- `crates/vxn-core-dsp` exists: `[lib] crate-type = ["rlib"]`, sole dependency
  `vxn-core-utils`, with the six empty module stubs (`control`, `declick`, `fx`,
  `env`, `os_region`, `test_util`), each doc-commented with the ticket that
  fills it. The crate doc carries the §4 rules, since that is where a future
  contributor will actually read them.
- Registered in the workspace `members` and `[workspace.dependencies]`
  ([Cargo.toml:6](../../Cargo.toml#L6), [:62](../../Cargo.toml#L62)).
- `cargo build -p vxn-core-dsp` green; `cargo build --workspace` green;
  `cargo clippy -p vxn-core-dsp --all-targets` 0 diagnostics.
- **Pure addition confirmed:** grepping `vxn-core-dsp` across every `Cargo.toml`
  in the tree returns only the workspace root and the crate's own manifest. No
  synth crate depends on it yet.
