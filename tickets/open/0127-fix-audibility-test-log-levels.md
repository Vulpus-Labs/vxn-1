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
