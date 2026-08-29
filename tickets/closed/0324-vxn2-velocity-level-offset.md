---
id: "0324"
product: vxn-2
title: "vxn-2: port ScaleVelocity — velocity as a signed level offset that can boost"
priority: high
created: 2026-08-29
epic: E048
depends: ["0323"]
---

## Summary

Ticket of [E048](../../epics/open/E048-log-domain-level-pipeline.md).
`vel_factor` is a linear multiplier that tops out at `1.0`:

```rust
1.0 - vs * (1.0 - v_curve)     // ceiling at unity when velocity = 127
```

The hardware's `ScaleVelocity` returns a **signed level offset** added to the
accumulator, so high velocity pushes an operator *above* its nominal level. At
`kvs 7` that is **+5.25 dB** at velocity 127. VXN2 is correspondingly under
hardware across the top half of the velocity range, scaling with sensitivity:

```
kvs=7                          at velocity 110, by sensitivity
 vel   hw dB  vxn2 dB  short    kvs   hw dB  vxn2 dB  short
 127     5.2      0.0    5.2      0     0.0      0.0    0.0
 110     2.6     -2.5    5.1      2     0.8     -0.6    1.4
 100     0.4     -4.2    4.5      4     1.5     -1.3    2.8
  64   -10.5    -11.9    1.4      6     2.2     -2.1    4.3
  32   -23.6    -23.9    0.3      7     2.6     -2.5    5.1
```

Below ~vel 64 the two agree within about a dB; it is the loud half that is
wrong. On a modulator, 5 dB of level is 5 dB of modulation index — this is why
`Electric Boogaloo`'s 14:1 tine (`op2`, the patch's only `kvs 7` operator) reads
dull, and why boosting its output level by hand appears to fix it.

## Design

```rust
// velocity_data: 64 entries, indexed by velocity >> 1
fn vel_level_offset(velocity: u8, sensitivity: u8) -> i32 {
    let vv = VELOCITY_DATA[(velocity.min(127) >> 1) as usize] as i32 - 239;
    ((sensitivity.min(7) as i32 * vv + 7) >> 3) << 4      // 1/32 level units
}
```

Added to the accumulator from 0323 *after* its `min(127)` ceiling and *before*
the `max(0)` floor — the hardware applies no second ceiling, which is what lets
velocity exceed nominal.

Watch the arithmetic shift on negatives: `>> 3` floors toward −∞ in both C and
Rust for signed types, so a direct port is faithful. Assert it.

## Acceptance criteria

- [ ] `VELOCITY_DATA` ported exactly; `vel_level_offset` asserted against the
      reference on every `(velocity, sensitivity)` pair — 128 × 8, integers, no
      tolerance.
- [ ] Enters the 0323 accumulator between the ceiling and the floor.
- [ ] `vel_factor` retired; no remaining linear velocity multiplier.
- [ ] `kvs = 0` is exactly velocity-independent (offset 0 for all velocities).
- [ ] A tine-brightness regression test: `Electric Boogaloo` at C4 / vel 110,
      the 15th-harmonic sideband rises by ~5 dB relative to the fundamental.
- [ ] Headroom above nominal actually survives to the lane loop — depends on
      0325; until that lands, note the clamp swallows it and gate the audible
      acceptance accordingly.

## Notes

Do not compensate for this in preset data. Boosting an operator's output level
is a constant where the error is velocity-dependent: it corrects loud notes and
over-brightens soft ones, flattening exactly the dynamic response `kvs` exists
to provide.

## Close-out (2026-08-29)

- `vel_level_offset` in [level.rs](../../vxn-2/crates/vxn2-dsp/src/level.rs):
  64-entry `VELOCITY_DATA`, signed, `const fn` so `MAX_ATTAINABLE_AMP` derives
  from the ladder. Asserted exactly across all 128 × 8 inputs
  (`level::tests::vel_level_offset_matches_hardware`), the negative-shift
  flooring included.
- Enters the accumulator between its ceiling and floor, which is what lets it
  exceed nominal. `vel_factor` retired (`grep` → 0 hits); velocity's law is
  shared by both level curves, being a property of the keyboard not the curve.
- `zero_sensitivity_ignores_velocity` and
  `vel_level_offset_is_monotone_in_velocity` pin the two structural properties.
- Tine regression:
  [velocity_brightness.rs](../../vxn-2/crates/vxn2-engine/tests/velocity_brightness.rs).
  `tine_brightness_tracks_velocity` measures the 15th harmonic of
  Electric Boogaloo (ratio-1 carrier × ratio-14 modulator) and requires the vel
  40 → 110 span to exceed 22 dB; it was ~14 dB before this ticket and is ~29 dB
  now. Monotonicity is asserted on the offset rather than the spectrum — algo 5
  puts three chains near that harmonic and they interfere, so the rendered ratio
  dips at some velocities without the law being violated.
