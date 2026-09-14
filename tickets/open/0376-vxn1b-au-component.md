---
id: "0376"
product: vxn-1b
title: "vxn-1b ships VXN1b.component"
priority: medium
created: 2026-09-10
epic: E051
depends: ["0374", "0375"]
---

## Summary

Repeat [0373](0373-vxn2-au-component.md) + [0374](0374-xtask-format-au.md) for
vxn-1b, whose wrapper project already exists and differs from vxn-2's only in
names and identifiers. Tenth ticket of
[E051](../../epics/open/E051-audio-unit-distribution.md).

Cheap by construction — and the fact that it is a near-verbatim copy is the
argument for [0377](0377-shared-wrapper-cmake.md), which follows.

## Acceptance criteria

- [ ] `vxn-1b/wrapper` builds `VXN1b.component` with subtype `Vx1b` per
      [ADR 0004](../../adrs/0004-au-component-identity.md).
- [ ] `cargo run --package vxn1b-xtask -- bundle --format clap,vst3,au --universal`
      produces all three, signed, and installs them.
- [ ] Force-load assertion (`labs.vulpus.vxn1b` present in the binary) and
      `auval -v aumu Vx1b Vlps` both pass.
- [ ] `VXN1b.vst3` unchanged.

## Notes

If `auval` surfaces anything here that 0375 did not, it is a genuine vxn-1b
difference (its parameter set and matrix routing diverge from vxn-2's) and
belongs in a follow-up ticket rather than being absorbed silently.
