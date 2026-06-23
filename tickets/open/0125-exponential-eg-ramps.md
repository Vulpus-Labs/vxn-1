---
id: "0125"
product: vxn-2
title: "Exponential EG ramps for the Exp curve path (log-domain march)"
priority: medium
created: 2026-06-23
epic: E026
depends: ["0124"]
---

## Summary

Third ticket of [E026](../../epics/open/E026-dx7-log-level-curve.md). 0123/0124
make the level *targets* logarithmic but the EG still **marches linearly in
amplitude** toward them, so a decay to a now-much-lower target stretches in time
and the attack/decay *shape* isn't DX7's. Implement log-domain marching
(linear-in-dB ramp → exponential amplitude) for the `Exp` curve, the
characteristic DX7 tapered envelope.

## Design

- `eg.rs`: when `curve == Exp`, march the EG state in log2 units (linear ramp
  toward a log2 target at a dB/sec rate); output amplitude = `2^(level)` once per
  control tick. Keep `eg.level` (amplitude) as the value the lane loop reads.
- Recalibrate rates for the log domain: `rate_to_*` so segment times feel like
  DX7 (R=0 ≈ 20 s, R=99 ≈ 4 ms full sweep), verified by listening.
- `Lin` path unchanged (linear-amplitude ramp toward square targets).

## Acceptance criteria

- [ ] `Exp` EG marches in log2 domain; amplitude is `exp2` of the marched value.
- [ ] Rate curve recalibrated; attack/decay times sane across R=0..99 (listening
      check on an e-piano pluck, a pad swell, a bass).
- [ ] `Lin` path bit-identical to pre-0125 behavior.
- [ ] Stays **control-rate scalar** — the `exp2` is per-op per-block, never in
      the per-sample lane loop. Confirm via idle + full-poly benches (no
      regression) and an asm/CPU spot-check.
- [ ] The `op{N}-level` matrix destination (writes `eg.level` as amplitude —
      `stack.rs ~696`) still behaves correctly with the log-domain state.

## Notes

`EgState` likely needs a log-domain state field plus a stored `max_amp` (the
linear ceiling = OL × ks × vel applied after `exp2`). Watch the matrix op-level
write path and `force_sustain`. This is the only part of the epic that touches
the per-sample-adjacent code, hence the explicit bench gate.
