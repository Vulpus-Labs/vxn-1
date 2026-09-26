---
id: "0390"
product: monorepo
title: "fast_tanh's x⁶ coefficient is 4 and the Padé it names has 1 — and the seam that puts in every saturator"
priority: medium
created: 2026-09-26
epic: null
depends: []
---

## Summary

`fast_tanh` ([math.rs:17](../../crates/vxn-core-utils/src/math.rs#L17)) is documented as a
Padé(5,6) rational and does not carry the Padé's denominator: the `x⁶` term has a coefficient of
**4** where the approximant has **1**. The consequences are a 1.5 % error where the curve is used
hardest and a **0.028 discontinuity** at the ±2.5 clamp, in the ladder, the BBD write stage, the
phaser feedback and the dynamics saturator alike.

Found while porting the dynamics kernel to the Voltage rack (`vm-pro` E014/0067), where the port
has to decide whether to reproduce the step. It should not have to: it is a bug here.

## What is wrong

```rust
x * (10395.0 + 1260.0 * x2 + 21.0 * x4) / (10395.0 + 4725.0 * x2 + 210.0 * x4 + 4.0 * x6)
//                                                                            ^^^ should be 1
```

Measured in double precision against `tanh`:

| | current, `4x⁶` | Padé, `x⁶` |
| --- | --- | --- |
| max abs error on [0, 1] | 1.5e−4 | **3.2e−10** |
| max abs error on [0, 2.5] | 1.47e−2 | **4.6e−6** |
| turns over at | x = 2.534, value 0.9719 | x = 4.373, value 0.99928 |
| step at the clamp | **0.0281** | **7.2e−4** |

So the curve walks 1.5 % *under* `tanh` as it approaches the clamp and then **jumps 0.028 upward**
at it — a discontinuity in the transfer curve at a fixed input level, about −31 dB relative to
full scale. Any signal crossing that level repeatedly gets a spray of harmonics from the *step*
rather than from the curve, which is not what a `tanh` saturator is for and is not what the
docstring describes.

**A second symptom, and it confirms the diagnosis.** `tanh_c`
([oscillator.rs:53](../../vxn-1b/crates/vxn-dsp/src/poly/oscillator.rs#L53)) is the branchless
SIMD twin, and its docstring says the two *share* the Padé(5,6) coefficients and should be kept in
sync. They are not in sync at the top: `tanh_c` clamps its **input** to ±2.5 and then evaluates,
so it saturates at **±0.9719**, while `fast_tanh` clamps its **output** to **±1.0**. The poly
ladder and the scalar paths therefore saturate 0.028 apart today. With the coefficient fixed and
the clamp moved to the turnover, the two agree to 7e−4 and the difference stops mattering.

## The fix

**Drop the 4 and move the clamp to where the rational peaks:**

```rust
const LIMIT: f32 = 4.3731;           // the turning point; the rational is 0.99928 there
if x >= LIMIT { return 1.0; }
if x <= -LIMIT { return -1.0; }
let x2 = x * x; let x4 = x2 * x2; let x6 = x4 * x2;
x * (10395.0 + 1260.0 * x2 + 21.0 * x4) / (10395.0 + 4725.0 * x2 + 210.0 * x4 + x6)
```

One multiply cheaper, 3200× more accurate over the range the saturators actually occupy, and the
seam lands where the derivative is already ~0 — so the clamp is nearly `C¹` as well as 40× smaller
in value. **The clamp is still required**: past the turnover the rational decays toward 0, so an
unguarded large input would come out *quiet*, which is the one failure mode worse than a kink.

`tanh_c` takes the same coefficient and the same limit, keeping the input-clamp form its lane loop
needs.

**The alternative, if a seam-free curve is wanted:** `tanh x = 1 − 2/(2^(2x·log₂e) + 1)` through
`fast_exp2`. No clamp, no seam, monotone, asymptote exact by construction; measured error 3.4e−9
using the degree-7 `exp2` polynomial. It costs an `exp2` and a divide against five multiplies and
a divide, and it does not vectorise the way `tanh_c`'s callers need, so the recommendation is the
coefficient fix for both forms and this noted as the option not taken.

## What it changes audibly

- **Distortion character wherever `|x| ≥ 2.5` was reached** — high resonance and high drive in the
  ladder, hot writes into the BBD, the dynamics saturator above about 11 dB of drive. Less
  harshness, because a step is a wideband event and the curve is not.
- **Level: almost nothing.** For the dynamics block, whose output is normalised by `1/tanh(k)`,
  the change is **≤0.13 dB and only for drive near 11–12 dB**; ≤0.006 dB above 14.6 dB.
- **The poly ladder gains 0.028 of headroom at full saturation** (0.9719 → 0.99928), which is the
  `tanh_c` discrepancy above being closed rather than a new change.

## Acceptance criteria

- [ ] `fast_tanh`'s denominator is the Padé's, the clamp is at the turnover, and the docstring
      states both the error bound and the size of the remaining step.
- [ ] `tanh_c` carries the same coefficient and the same limit, and its docstring's "keep the two
      in sync" claim is true at the top of the range as well as in the middle.
- [ ] A test pins the maximum error against `f64::tanh` over [0, 2.5] and over [0, LIMIT], rather
      than only checking the endpoints as `tanh_key_points` does.
- [ ] A test pins the step at the clamp, so the next person to move it has to mean it.
- [ ] A test asserts `fast_tanh` and `tanh_c` agree to a stated tolerance across [−6, 6],
      including past the clamp. Neither exists today, which is why they drifted.
- [ ] Whatever currently pins the ladder, the BBD, the phaser and the dynamics kernel is re-run;
      bit-exactness assertions that move are re-baselined *deliberately*, each with a line saying
      the curve changed and by how much.
- [ ] A release note: patches with high drive or high resonance will sound slightly cleaner.

## Notes

- **Blast radius.** Four scalar call sites — `filter.rs:343` (ladder integrators),
  `delay_line.rs:473` (BBD write saturation), `dynamics.rs:322-325`, `phaser.rs:223` (feedback) —
  plus `tanh_c` in the poly oscillator's ladder and diode ring.
- **The ladder reaches it first.** `filter.rs:343` saturates the stage input, which carries
  `drive · x` minus the feedback, so with a unit-scale voice the step fires at `drive ≥ 2.5` — well
  inside the 0.1..4 range `PARAMETERS.md` gives `drive`, and reachable at lower drive as soon as
  resonance contributes. It is also *inside the feedback loop*, so the clamp is what sets the
  self-oscillation limit cycle: fixing the coefficient moves the oscillation's amplitude and timbre,
  not just the distortion of a signal passing through. That is the change most likely to be heard on
  existing patches, and the one the re-baselining criterion above is really about.
- **How hot a BBD write has to be to reach it.** `chorus.rs`'s `SAT_DRIVE` is 1.2 and the
  saturation is applied to the *filtered* write signal, so the step needs `|filtered| ≥ 2.0833` —
  6.4 dB above unit full scale. Reachable in the plugin, where the chorus sits ahead of the master
  limiter and two layer levels can sum past 1.0; in the Voltage port it needs 10.42 V into
  `JanusBBD`, which the Juno chain reaches only when the voice sum itself is over full scale. At
  the crossing the write output steps by 0.0234 (≈ −32.6 dB), four times per cycle on a sine,
  broadband, in a base-rate path.
- **Downstream, outside this repository.** `vm-pro` has two verbatim copies of the current curve:
  `janus-vcf`'s `FastTanh` and `janus-chorus`'s `BbdDelayLine.softClip`. Both are ports whose
  tests exist to reproduce *this* repository's sound, and both javadocs note the seam sits "about
  0.013 below `Math.tanh`" — measuring the approach and missing the jump. They do not have to
  follow this fix immediately, and E014/0067 records that decision separately; what matters is
  that they can no longer follow it by accident.
- **RMS error < 0.05 over [−3, 3]**, which `tanh_c`'s docstring claims as its accuracy, is an
  order of magnitude looser than either variant deserves and is worth recomputing while the
  coefficients are in hand.
