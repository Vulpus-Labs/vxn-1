---
id: "0127"
product: vxn-2
title: "Fix every_param_sweep_is_audible under the log level curve"
priority: medium
created: 2026-06-23
epic: E026
depends: ["0123"]
---

## Summary

Fifth ticket of [E026](../../epics/open/E026-dx7-log-level-curve.md). The
`every_param_sweep_is_audible` regression guard
(`vxn-2/crates/vxn2-engine/tests/param_audibility.rs:434`) fails under the log
curve: its test patch has modulators sustaining at L3=50, which is now ≈ −37 dB,
so sweeping those operators' params barely changes the output (rel-diff < 1e-4).
This is a **fixture mismatch, not severed wiring** — the modulators are now
correctly quiet.

## Design

Give the swept modulators an audible contribution in the test context: raise the
test patch's modulator EG sustain (e.g. L3=99) and/or add per-param context
overrides so each wired param still produces an audible delta when swept.

## Acceptance criteria

- [ ] Remove the `#[ignore = "E026/0127: ..."]` on `every_param_sweep_is_audible`
      (added when the log curve landed to keep main green) and make it pass.
- [ ] `every_param_sweep_is_audible` green under the default (Exp) curve.
- [ ] Failing set re-verified as fixture-only — confirm none is a real wiring
      regression. (Was: `op2-*`, `op6-eg-*`, `feedback`, `mtx3/mtx7-depth`.)
- [ ] The thorough variant (`..._thorough`, `#[ignore]`) still passes when run.

## Notes

The test framework already supports per-param context overrides (the failure
message references them). Coordinate with 0123 (curve) and 0124 (the patch may be
built with explicit `Exp`). Keep the guard meaningful — don't just lower the
1e-4 threshold.

## Close-out (2026-06-24)

- `#[ignore]` + the "TEMPORARILY DISABLED" comment removed from
  `every_param_sweep_is_audible`
  ([param_audibility.rs](../../vxn-2/crates/vxn2-engine/tests/param_audibility.rs));
  it runs in the default suite again and passes.
- Root cause confirmed fixture-only: under the log curve the default-patch
  modulator ops sustain at ≈ −37 dB, so in `base_context` (algo 32) op2 was
  inaudible across **all** its params and op6 / `feedback` / `mtx3-depth`
  (PitchEg→Op2Level) / `mtx7-depth` (Lfo1→Op6Pitch) fell below `AUDIBLE_EPS`.
  Fix: `base_context` now pins every op to a full-level carrier
  (`op{n}-level = 99`, `op{n}-eg-l3 = 90`) so each op's params move the mix
  directly — wiring was intact, the context was just too quiet.
- `AUDIBLE_EPS` (1e-4) **unchanged** — guard kept meaningful; the fix raises
  signal, not lowers the bar. The per-param `-eg-` override still re-shapes the
  op under test (it only overrides the baseline sustain).
- Verified: full table sweep green under the default (Exp) curve (`run_sweep(1)`,
  ~9 s); the previously-inert set (`op2-*`, `op6-eg-*`, `op6-ks-rate`,
  `feedback`, `mtx3/mtx7-depth`) all now exceed the floor.
- `every_param_sweep_is_audible_thorough` (`#[ignore]`, 3× windows) still passes
  when run with `--ignored` (~26 s).
