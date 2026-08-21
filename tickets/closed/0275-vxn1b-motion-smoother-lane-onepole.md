---
id: "0275"
product: vxn-1b
title: "MotionSmoother is five hand-rolled copies of one lane one-pole"
priority: medium
created: 2026-08-21
epic: null
depends: []
---

## Summary

[mod_smoothing.rs](../../vxn-1b/crates/vxn1b-engine/src/mod_smoothing.rs) carries
five independent smoothed quantities — `pwm1`, `pwm2`, `xmod`, `pan`,
`amp_stat` — and every one is the same one-pole over a `[f32; N]` lane array:

```rust
state[v] += coeff * (target - state[v]);
```

Each has been given its own hand-written `snap` / `active` / `tick` / `current`
quadruple, so the file holds roughly fifteen near-identical methods. `pan` and
`xmod` are structurally *identical* — same `slow_coeff`, single lane array — and
differ only in their doc prose. `reset()` is a seven-line list that must be
extended by hand, and the same for `new()`.

The tax lands on every new destination: 0260 (Pan) and 0242 (cross-mod) each had
to add a field, a `new` entry, a `reset` entry and four methods, and `Pan` needed
its own `snap_pan` separate from `snap_slow` for a reason
([mod_smoothing.rs:168](../../vxn-1b/crates/vxn1b-engine/src/mod_smoothing.rs#L168) —
a stolen lane must not glide across the image) that is a *call-site* concern, not
a structural one.

The pitch cascade is genuinely different (two stages, its own coefficient, a
documented C1-continuity rationale) and stays as it is.

## Design

Introduce a private lane-array one-pole and make the smoothed quantities fields
of it:

```rust
#[derive(Clone, Copy, Debug, Default)]
struct LaneOnePole([f32; N]);

impl LaneOnePole {
    #[inline] fn snap(&mut self, v: usize, t: f32) { self.0[v] = t }
    #[inline] fn current(&self, v: usize) -> f32 { self.0[v] }
    #[inline] fn tick(&mut self, v: usize, t: f32, a: f32) -> f32 {
        self.0[v] += a * (t - self.0[v]);
        self.0[v]
    }
    #[inline] fn active(&self, v: usize, t: f32) -> bool {
        (t - self.0[v]).abs() > SETTLE_EPS || self.0[v].abs() > SETTLE_EPS
    }
    #[inline] fn settled(&self, v: usize, t: f32) -> bool {
        (self.0[v] - t).abs() <= SETTLE_EPS
    }
}
```

Then `pwm: [LaneOnePole; 2]`, `xmod`, `pan`, `amp_stat` as fields; `reset()`
becomes assignment of `Default::default()`; the coefficient stays at the
`MotionSmoother` level (`slow_coeff` for pwm/xmod/pan, `amp_coeff` for
`amp_stat`) and is passed into `tick`.

Keep the **public** surface as the bank sees it today — `tick_pwm`, `pan_active`,
`amp_stat_settled`, … — as thin wrappers that pick the field and the coefficient.
That keeps this ticket a pure internal collapse and leaves the call-site reshape
to [0276](0276-vxn1b-bank-render-decomposition.md).

Where a doc comment carries a real decision rather than a description — the
`snap_pan`-is-separate rationale, the pwm "sum before the one-pole" note (0261),
the xmod "patch amount rides on top" note (0242) — keep it on the wrapper. Drop
the prose that only restated the arithmetic.

## Acceptance criteria

- [x] `state[v] += coeff * (target - state[v])` appears once for the one-pole
      family (the pitch cascade's two stages are separate and stay).
- [x] `reset()` and `set_sample_rate()` cannot go stale when a smoothed dest is
      added — no per-field list to extend.
- [x] The `MotionSmoother` methods `bank.rs` calls keep their current names and
      signatures; `bank.rs` is unchanged by this ticket.
- [x] Existing smoothing tests pass unchanged, including
      `cascade_output_slope_starts_at_zero` and `cascade_converges_to_target`.
- [x] Default-patch render is bit-identical.

## Notes

Roughly halves the file with no behaviour change. Land before
[0276](0276-vxn1b-bank-render-decomposition.md), which reshapes the call sites on
top of it.

## Close-out

Landed 2026-08-21. Files touched: `vxn1b-engine/src/mod_smoothing.rs`.

`LaneOnePole([f32; N])` with `snap`/`current`/`tick`/`active`/`settled` replaces
the five hand-written copies. Fields are now `pwm: [LaneOnePole; 2]`, `xmod`,
`amp_stat`, `pan`; the coefficient stays on `MotionSmoother` and is passed into
`tick`, because it belongs to the tier (`slow_coeff` per quantum, `amp_coeff`
per frame) rather than to the quantity. `new()` and `reset()` assign
`Default::default()` wholesale, so neither can go stale when a dest is added.
The pitch cascade is untouched.

The public surface kept its names and signatures, so `bank.rs` was unchanged by
this ticket, as designed.

All acceptance criteria met. One claim from the Notes was wrong, though:

- **"Roughly halves the file"** — it did not. 418 → 479 lines. The recurrence
  went from five copies to one, but the documented public wrappers are most of
  the file and the ticket also (correctly) required keeping them. The estimate
  ignored that. The structural win is real; the line-count prediction was not.

Bit-identity confirmed on all four patches.
