---
id: "0231"
product: monorepo
title: "Delay unification — shared vxn-2-superset StereoDelay with optional feedback damping; vxn-1 + vxn-1b adopt"
priority: medium
created: 2026-08-02
epic: E041
depends: ["0227"]
---

## Summary

Fourth ticket of [E041](../../epics/open/E041-shared-fx-unification.md). The
vxn-1/vxn-2 delays differ at feature-set level, not sonic identity
(established during planning): interp (linear vs Catmull-Rom), time smoothing
(40 ms one-pole slew vs 100 ms `Smoothed` glide — same audible continuous
glide), feedback path (one-pole damping param vs fixed 10 Hz DC blocker), mix
law, optional-vs-hardwired ping-pong, sync. Unify on a vxn-2-superset kernel
in `vxn-core-dsp::delay`:

- Base: [vxn2-dsp/src/delay.rs](../../vxn-2/crates/vxn2-dsp/src/delay.rs)
  (Ring + cubic read, DC blocker, sync, pingpong flag, ~100 ms time glide,
  `on`/`mix_primed`).
- Added: optional feedback damping (vxn-1's param), **gated so
  `damping == 0.0` skips the filter entirely** — one-pole with a=0 is not
  float-identity (`lp + (wet-lp)` ≠ `wet`), so the gate is what keeps vxn-2's
  render hash bit-exact.

## Acceptance criteria

- [ ] Move commit: shared kernel, vxn-2 hash unchanged (damping-gate verified
      by the hash itself + a unit test that damping==0 is bit-exact vs the
      pre-move kernel).
- [ ] Adoption commit (vxn-1 + vxn-1b together): pingpong=true, damping param
      mapped, equal-power → linear mix, 40 ms slew → 100 ms glide; outer
      `delay_fade` + DELAY slot fade deleted;
      [vxn-dsp/src/delay.rs](../../vxn-1/crates/vxn-dsp/src/delay.rs) retired
      (or shimmed until nothing imports it).
- [ ] `REBASELINE:` commit: vxn-1 delay_toggle declick + the delay-time-sweep
      test (its slew-vs-snap comparison re-anchored to the 100 ms glide) +
      baselines; A/B notes; Reaper sign-off.
- [ ] vxn-3's send-bus delay untouched (out of scope; may adopt later with a
      saturating-feedback option).

## Notes

vxn-1's engine currently snaps `DelayTime` because the ramp lives in-kernel
([delay.rs:6-12](../../vxn-1/crates/vxn-dsp/src/delay.rs#L6-L12)) — that
contract carries over unchanged (glide still lives in-kernel), only the
constant/curve differs. Sync plumbing on vxn-1 is optional scope: wire
`sync=off` initially, sync exposure is a separate feature decision.
