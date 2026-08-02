---
id: "0227"
product: monorepo
title: "Pure-move kernels into vxn-core-dsp: DynamicsBlock, HpfKernel, scalar OTA ladder + OtaLadderCoeffs"
priority: high
created: 2026-08-02
epic: E040
depends: ["0224", "0226"]
---

## Summary

Final ticket of [E040](../../epics/open/E040-vxn-core-dsp-foundations.md).
Move the three kernels that are already duplicates:

- **DynamicsBlock** — vxn-1's
  [dynamics.rs](../../vxn-1/crates/vxn-dsp/src/dynamics.rs) header says
  "Ported verbatim from vxn-2"; measured diff is import path + test helpers
  only, kernel byte-identical. Move to `vxn-core-dsp::dynamics`; both
  `vxn-dsp/src/dynamics.rs` and
  [vxn2-dsp/src/dynamics.rs](../../vxn-2/crates/vxn2-dsp/src/dynamics.rs)
  become re-export shims.
- **HpfKernel** — TPT one-pole, effectively identical forks
  ([vxn-dsp/src/hpf.rs](../../vxn-1/crates/vxn-dsp/src/hpf.rs) vs
  [vxn2-dsp/src/hpf.rs](../../vxn-2/crates/vxn2-dsp/src/hpf.rs)). Move the
  scalar kernel; vxn-1's 8-wide `PolyHpf` stays in vxn-dsp (SoA body).
- **Scalar OTA ladder** — `OtaLadderKernel` / `OtaLadderCoeffs` /
  `FilterMode` / `FilterSlope` / mix tables from
  [vxn2-dsp/src/filter.rs](../../vxn-2/crates/vxn2-dsp/src/filter.rs) (+ the
  coefficient half of
  [vxn-dsp/src/ota_ladder.rs](../../vxn-1/crates/vxn-dsp/src/ota_ladder.rs)).
  `OtaLadderCoeffs::new` takes the 0226 `OsRate` newtype; `k_cap` stays
  absolute Hz with the rationale documented at the constructor.
  [poly/ladder.rs](../../vxn-1/crates/vxn-dsp/src/poly/ladder.rs) imports
  Coeffs/modes; its SoA body + `with_mix!` markers do NOT move.

## Acceptance criteria

- [ ] Three kernels in vxn-core-dsp, shims in place, zero import churn inside
      synth crates.
- [ ] DynamicsBlock's WetFade adoption (replacing its hand-rolled
      enabled/mix/mix_primed trio) lands as a **separate commit**, kept only
      if hashes stay byte-identical; else deferred to E041.
- [ ] vxn-1 baseline, vxn-1b parity, vxn-2 render hash, dynamics_integration,
      filter_integration, all bit-exact-passthrough tests: unchanged.
- [ ] asm-check: `poly/ladder` monomorph NEON counts unchanged; `filter_path`
      bench within noise.

## Notes

Ladder rate split preserved: coefficients cooked at OS rate, `tick_coeffs` at
base rate, `process` at OS rate — the 4-call protocol is untouched here
(E043/0237 later wraps its increments in `CoeffRamp`). Related:
[[vxn1-ota-filter-perf]], [[vxn1-soa-match-defeats-simd]].
