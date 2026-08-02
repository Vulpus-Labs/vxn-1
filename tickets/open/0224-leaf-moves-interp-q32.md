---
id: "0224"
product: monorepo
title: "Leaf moves to vxn-core-utils: HalfbandInterp/Interpolator + Q32 phase constants"
priority: medium
created: 2026-08-02
epic: E040
depends: ["0222"]
---

## Summary

Third ticket of [E040](../../epics/open/E040-vxn-core-dsp-foundations.md). The
decimator half of the halfband pair already lives in
[vxn-core-utils/src/halfband.rs](../../crates/vxn-core-utils/src/halfband.rs);
the interpolating half (`HalfbandInterp`, `Interpolator`) is stranded in
[vxn2-dsp/src/halfband.rs](../../vxn-2/crates/vxn2-dsp/src/halfband.rs) for
historical reasons (only vxn-2 upsamples). Move it next to the decimator.
Likewise the Q32 phase convention (`PM_SCALE_Q32`, inc/wrap helpers) is
triplicated across [op.rs](../../vxn-2/crates/vxn2-dsp/src/op.rs#L109),
`lfo.rs`, and `stack.rs` — dedupe into `vxn-core-utils::math`.

## Acceptance criteria

- [ ] `HalfbandInterp` + `Interpolator` in `vxn-core-utils::halfband`;
      `vxn2-dsp/src/halfband.rs` reduced to a re-export shim (pattern:
      [smoother.rs](../../vxn-2/crates/vxn2-dsp/src/smoother.rs)).
- [ ] Q32 consts/helpers defined once in `vxn-core-utils::math`; `op.rs` /
      `lfo.rs` / `stack.rs` import them; no duplicate definitions remain.
- [ ] vxn-2 render hash, vxn-1b parity, halfband unit tests (incl.
      `roundtrip_latency_base_samples`) all byte-identical — pure move.

## Notes

vxn-1 keeps its normalised-f32 phase convention — Q32 helpers are shared for
vxn-2 (and future synths), not forced on vxn-1. Shims keep in-crate import
paths stable, zero downstream churn.
