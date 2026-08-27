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

## Close-out (2026-08-27)

- `HalfbandInterp` + `Interpolator` moved into
  [vxn-core-utils::halfband](../../crates/vxn-core-utils/src/halfband.rs),
  beside the decimator they already shared a tap table with. Their 5 unit tests
  moved with them, folded into that module's existing `mod tests` rather than
  declaring a second one.
- [vxn2-dsp/src/halfband.rs](../../vxn-2/crates/vxn2-dsp/src/halfband.rs) is now
  a 13-line re-export shim on the `smoother.rs` pattern — 336 lines to 13.
  In-crate `vxn2_dsp::halfband::…` paths still resolve, so no downstream churn.
- Q32 vocabulary defined once in
  [vxn-core-utils::math](../../crates/vxn-core-utils/src/math.rs):
  `Q32_PER_CYCLE` (f32), `Q32_PER_CYCLE_F64`, `INV_Q32_PER_CYCLE`, plus
  `q32_to_unit` / `q32_to_unit_f64` / `phase_inc_q32`.
- Seven vxn-2 sites repointed, across four files rather than the three the
  ticket named — `sine.rs` had two as well as `op.rs` / `lfo.rs` / `stack.rs`:
  `PM_SCALE_Q32` and `U32_PER_CYCLE` are now re-exports of the shared consts,
  `INV_PM_SCALE_Q32` is a re-export of `INV_Q32_PER_CYCLE`, and the five bare
  `4_294_967_296.0` literals are gone. Grep for that literal under `vxn-2/`
  returns **0**.

### Both precisions kept, deliberately

The sites were not all the same expression: the audio-rate ones do an **f32
reciprocal multiply** (`phase as f32 * (1.0 / 2^32)`) and the test oracle in
`sine.rs` does an **f64 divide** (`phase as f64 / 2^32`). Those are not
interchangeable, and collapsing them to one helper would have moved the render
hash. Both forms are provided and each call site kept the one it had.
`helpers_match_the_literal_expressions_they_replaced` pins each helper against
the literal expression it replaced, so a later tidy-up that swaps a divide for a
multiply — or narrows the f64 form — fails there rather than in a render hash
weeks later.

### Verification

- **vxn-2 render hash bit-exact.** Same method as 0225: run with
  `VXN_RENDER_HASH=1` and compare actuals, since the recorded constant is pinned
  to CI's macos-15 and this is macOS 14. Result **`0x533a37a7def1921a`** —
  identical to the clean-worktree HEAD capture. The change moves nothing.
- Halfband unit tests including `roundtrip_latency_base_samples` pass unmodified
  in their new home.
- New tests: the three constants agree and the reciprocal is exact (2^32 is a
  power of two, so `1/2^32` is representable with no rounding); the endpoints map
  as documented.

### Caveats and out-of-scope findings

- **"Vectorisation unchanged" is reasoned, not measured.** `stack.rs` is SoA hot
  path, but the edit is a constant swapped for an identical-valued constant with
  no control-flow change, so codegen should be unaffected. The asm-check harness
  that would *prove* it is [0223](0223-asm-check-perf-baselines.md), which has
  not landed yet — this ticket runs ahead of its own measurement tool. Worth a
  confirming asm-check pass once 0223 exists.
- **vxn-3 has an eighth copy**: `PHASE_SCALE: f32 = 4_294_967_296.0` in
  [vxn3-dsp/src/sine.rs:9](../../vxn-3/crates/vxn3-dsp/src/sine.rs#L9). Left
  alone — `vxn3-dsp` has no `vxn-core-utils` dependency, and adding a dependency
  edge to share one constant is a bigger change than this ticket's scope. Fold
  it in when vxn3-dsp next needs anything else from core-utils.
