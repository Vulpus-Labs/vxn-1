---
id: "0085"
product: vxn-1
title: vxn-dsp — gate dead mono kernels, split poly.rs
priority: low
created: 2026-06-21
epic: E024
---

## Summary

Two structural tidies in vxn-dsp. Behaviour-preserving.

1. **Dead mono kernels exposed as public API.** `Oscillator`
   (`oscillator.rs`), `OtaLadderKernel` (`ota_ladder.rs`),
   `HpfKernel` (`hpf.rs`), and `MonoPhaseAccumulator`
   (`phase.rs`) have zero external callers — the engine uses
   only the `Poly*` kernels. They survive only as their own
   modules' differential test oracles, yet are `pub` and
   re-exported from `lib.rs:64-71`, so a consumer can't tell
   which kernels are real. They also accrete maintenance:
   each must track the poly arithmetic for a
   documentation-only payoff (this is the third copy behind
   the waveform-formula triplication the review flagged).

2. **`poly.rs` is a 1820-line god-module.** It houses two
   unrelated SoA kernels (oscillator+sync+PM+sub+ring, and
   the OTA ladder) plus their marker-type machinery and
   ~800 lines of tests — the one file that violates the
   crate's otherwise-tidy one-concept-per-module convention.

## Acceptance criteria

- [ ] `Oscillator`, `OtaLadderKernel`, `HpfKernel`,
      `MonoPhaseAccumulator` are moved behind `#[cfg(test)]`
      or `pub(crate)` and dropped from the `lib.rs:64-71`
      re-export list — kept strictly as test oracles, or
      deleted if the poly versions are self-documenting
      enough. Their differential tests against the poly
      kernels still run.
- [ ] `poly.rs` is split into `poly/oscillator.rs`
      (`PolyOscillator` + `WaveKind` + sub + ring) and
      `poly/ladder.rs` (`PolyOtaLadder` + `LadderMix`),
      re-exported from a thin `poly/mod.rs`; the
      `needless_range_loop` allow and the SIMD-rationale
      doc comments move with the code they justify.
- [ ] `cargo test -p vxn-dsp` green (incl. the scalar-vs-poly
      differential oracles and FFT spectral tests);
      `tests/baseline.rs` render hash unchanged.

## Notes

Distinct from E011 **0019**, which adds the duplicated-tanh
cross-reference comments and demotes `HALF_SEMITONE_VOCT` —
this ticket does the dead-kernel gating and the file split,
not the tanh/const items. Do NOT merge the tanh
implementations (branch/branchless split is deliberate,
memory `vxn1-tanh-branchless-only`). Mechanical, low-risk;
asm verification is not needed (nothing touches the hot
path's emitted code, and per-crate asm is misleading
pre-LTO — memory `vxn1-ota-filter-perf`).
