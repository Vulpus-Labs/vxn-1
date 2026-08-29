//! The operator level accumulator — [ADR 0010].
//!
//! Every contributor to an operator's amplitude combines by **addition in level
//! units** (~0.75 dB each), with the hardware's two clamps in the hardware's
//! order, and exactly one conversion to linear amplitude at the end:
//!
//! ```text
//!   units  = scale_outlevel(OL)          // 0..99 → 0..127
//!   units += ks_level_offset(…)          // key level scaling
//!   units  = min(127, units)             // ceiling, BEFORE velocity
//!   frac   = units × 32                  // → 1/32-unit resolution
//!   frac  += vel_level_offset(…)         // signed; may push above full scale
//!   frac   = max(0, frac)                // floor only — no second ceiling
//!   amp    = 2^((frac − FULL_SCALE_FRAC) / (32 × 8))
//! ```
//!
//! Contributors may only be *added*. That is the point: the three calibration
//! bugs this replaces were each a contributor applied as an independent linear
//! multiplier — a fade toward the keyboard edge, a velocity curve that could
//! only attenuate, a ceiling expressed as `.min(1.0)` on a product. None of
//! them is expressible as a signed level offset.
//!
//! The final conversion is `exp2`, not a table. It runs at control rate, where
//! it is free and *more* accurate than the hardware's lookup; reproducing that
//! table's error would be imitation rather than fidelity. Tables are ported
//! where their **quantisation is audible** — [`scale_outlevel`] here,
//! `ScaleCurve` in [`crate::ks`].
//!
//! [ADR 0010]: ../../../adrs/0010-log-domain-level-pipeline.md

/// Level units per octave. The domain is ~0.75 dB/step, so 8 steps make 6.02 dB
/// — the same scale as [`crate::eg::level_to_amp`] (ADR 0007).
pub const UNITS_PER_OCTAVE: f32 = 8.0;

/// Sub-unit resolution of the accumulator: the hardware's post-`<<5` domain,
/// 1/32 of a level unit (~0.0235 dB). Velocity offsets arrive at this scale.
pub const FRAC_PER_UNIT: i32 = 32;

/// Ceiling on the summed level, applied *before* velocity. An operator already
/// at full output takes no boost from key scaling.
pub const MAX_UNITS: i32 = 127;

/// Accumulator value denoting amplitude `1.0` — an operator at nominal full
/// output, before velocity.
pub const FULL_SCALE_FRAC: i32 = MAX_UNITS * FRAC_PER_UNIT;

/// The largest amplitude the accumulator can produce: nominal plus the biggest
/// velocity boost the ladder allows (`vel-sens 7` struck at 127), ≈ 1.833.
///
/// Velocity is signed and takes no ceiling above, so a hard strike genuinely
/// exceeds nominal — that is the dynamic response, not an overflow. The engine
/// bounds the rendered level against **this**, not against 1.0.
///
/// Rejected alternative: renormalise so that this value *is* 1.0, putting
/// nominal at 0.546. It looks tidier and keeps the old bound, but the level-
/// dependent stages — the filter's drive, the dynamics saturator — sit between
/// the operators and the master volume, so a uniform 5.25 dB cut at the source
/// moves their operating point and no master-volume sweep can put it back. It
/// would have quietly re-voiced the compressor on all 45 presets.
///
/// Derived from the ladder rather than written as a literal.
pub const MAX_ATTAINABLE_AMP: f32 = 1.8340081;

/// Compile-time check that [`MAX_ATTAINABLE_AMP`] tracks the velocity ladder.
/// `exp2` is not `const`, so the constant is spelled out and pinned by a test
/// ([`tests::max_attainable_amp_tracks_the_ladder`]) rather than computed here.
const _: () = assert!(vel_level_offset(127, 7) == 224);

/// Output-level curve below the knee. Above `OL 20` the hardware is linear
/// (`28 + OL`); below it this table compresses, so quiet operators fall away
/// faster than a uniform 0.75 dB/step would take them.
const LEVELLUT: [i32; 20] = [
    0, 5, 9, 13, 17, 20, 23, 25, 27, 29, 31, 33, 35, 37, 39, 41, 42, 43, 45, 46,
];

/// Operator level (0..99) → level units (0..127). Serves the operator output
/// level *and* the EG's L-values ([`crate::eg::level_to_amp`]), which is how the
/// hardware does it — one `scaleoutlevel` for both.
///
/// Above the knee this is ADR 0007's curve exactly — `28 + OL` against a full
/// scale of 127 is `2^((OL−99)/8)`, which is why 0.75 dB/step matched the
/// hardware without per-patch fudging. Below `OL 20` the table compresses
/// (−95.6 dB at OL 0 against the straight line's −74.5 dB), so a very quiet
/// operator, or an EG segment resting at a low L, falls away faster.
#[inline]
pub fn scale_outlevel(level: u8) -> i32 {
    let l = level.min(99) as i32;
    if l >= 20 { 28 + l } else { LEVELLUT[l as usize] }
}

/// Velocity curve. Indexed by `velocity >> 1`; the reference offsets it by 239
/// so that the mid-table sits near zero and the top of the range is *positive*
/// — velocity's contribution is signed, and at high sensitivity it boosts an
/// operator above its nominal level rather than merely failing to attenuate it.
const VELOCITY_DATA: [u8; 64] = [
    0, 70, 86, 97, 106, 114, 121, 126, 132, 138, 142, 148, 152, 156, 160, 163, 166, 170, 173,
    174, 178, 181, 184, 186, 189, 190, 194, 196, 198, 200, 202, 205, 206, 209, 211, 214, 216,
    218, 220, 222, 224, 225, 227, 229, 230, 232, 233, 235, 237, 238, 240, 241, 242, 243, 244,
    246, 246, 248, 249, 250, 251, 252, 253, 254,
];

/// Velocity's contribution to the level accumulator, in 1/32 level units.
/// Signed: negative for soft playing, **positive** at high velocity and
/// sensitivity — `+5.25 dB` at sensitivity 7, velocity 127.
///
/// This is the only contributor allowed to push the accumulator above full
/// scale; hardware applies no ceiling after it, only the floor. A `const fn` so
/// [`FULL_SCALE_FRAC`] can be derived from the ladder rather than written as a
/// literal.
///
/// The `>> 3` floors toward −∞ on negative values in both Rust and the
/// reference (arithmetic shift on a signed type), so soft velocities port
/// directly. Asserted across the whole input space.
#[inline]
pub const fn vel_level_offset(velocity: u8, sensitivity: u8) -> i32 {
    let v = if velocity > 127 { 127 } else { velocity };
    let s = if sensitivity > 7 { 7 } else { sensitivity } as i32;
    let vv = VELOCITY_DATA[(v >> 1) as usize] as i32 - 239;
    ((s * vv + 7) >> 3) << 4
}

/// A frac-resolution offset → the linear gain it represents. For the `Lin`
/// escape hatch, which still applies its offsets as multipliers.
#[inline]
pub fn frac_to_gain(frac: i32) -> f32 {
    (frac as f32 / (FRAC_PER_UNIT as f32 * UNITS_PER_OCTAVE)).exp2()
}

/// A level offset in whole units → the linear gain it represents. For the `Lin`
/// escape hatch, whose square level curve never joined the level domain and so
/// still applies key scaling as a multiplier.
#[inline]
pub fn units_to_gain(units: i32) -> f32 {
    (units as f32 / UNITS_PER_OCTAVE).exp2()
}

/// Accumulator value → linear amplitude. The single conversion out of the level
/// domain.
#[inline]
pub fn frac_to_amp(frac: i32) -> f32 {
    ((frac - FULL_SCALE_FRAC) as f32 / (FRAC_PER_UNIT as f32 * UNITS_PER_OCTAVE)).exp2()
}

/// Cook an operator's amplitude ceiling from its output level plus the level
/// offsets that act on it. `ks_offset` is in whole level units (see
/// [`crate::ks::ks_level_offset`]); `vel_offset` is in 1/32 units, and is the
/// only contributor permitted to push the total above full scale.
///
/// `OL 0` is hard silence, matching ADR 0007 — the hardware's own floor there is
/// −95.6 dB rather than true zero, but the codebase has always promised exact
/// silence and nothing is gained by walking that back.
#[inline]
pub fn op_max_amp(level: u8, ks_offset: i32, vel_offset: i32) -> f32 {
    if level == 0 {
        return 0.0;
    }
    let units = (scale_outlevel(level) + ks_offset).min(MAX_UNITS);
    frac_to_amp((units * FRAC_PER_UNIT + vel_offset).max(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eg::{EgCurve, level_to_amp};

    /// Exact against the reference for every input, table and linear branch
    /// alike — integers, so no tolerance.
    #[test]
    fn scale_outlevel_matches_hardware() {
        const LUT: [i32; 20] = [
            0, 5, 9, 13, 17, 20, 23, 25, 27, 29, 31, 33, 35, 37, 39, 41, 42, 43, 45, 46,
        ];
        for l in 0..=99u8 {
            let want = if l >= 20 { 28 + l as i32 } else { LUT[l as usize] };
            assert_eq!(scale_outlevel(l), want, "scale_outlevel({l})");
        }
        assert_eq!(scale_outlevel(99), MAX_UNITS, "OL 99 is full scale");
    }

    /// The accumulator and the EG's level curve are **one** curve, at every
    /// level — ADR 0007 §1's "one logarithmic level curve, shared by EG levels
    /// and operator output level", which held above the knee only until the EG
    /// joined this ladder too.
    #[test]
    fn agrees_with_the_eg_level_curve() {
        // Referred to nominal, since 0325 put full scale above it.
        let nominal = op_max_amp(99, 0, 0);
        for l in 1..=99u8 {
            let via_units = op_max_amp(l, 0, 0) / nominal;
            let via_curve = level_to_amp(l, EgCurve::Exp);
            let ratio_db = 20.0 * (via_units / via_curve).log10();
            assert!(ratio_db.abs() < 1e-3, "L {l}: {ratio_db:.4} dB apart");
        }
        assert_eq!(op_max_amp(0, 0, 0), 0.0, "OL 0 is hard silence");
        assert_eq!(level_to_amp(0, EgCurve::Exp), 0.0, "L 0 is hard silence");
    }

    /// What the knee is worth, against the straight `2^((L−99)/8)` line the EG
    /// used to draw. Pinned because it is exactly the amount every low sustain
    /// in the bank moves by.
    #[test]
    fn the_knee_is_what_the_straight_line_missed() {
        let straight = |l: u8| 2_f32.powf((l as f32 - 99.0) / 8.0);
        let db = |l: u8| 20.0 * (op_max_amp(l, 0, 0) / straight(l)).log10();
        assert!((db(19) - -0.8).abs() < 0.15, "L 19: {}", db(19));
        assert!((db(12) - -3.8).abs() < 0.15, "L 12: {}", db(12));
        assert!((db(4) - -11.3).abs() < 0.15, "L 4: {}", db(4));
        for l in 20..=99u8 {
            assert!(db(l).abs() < 1e-3, "L {l} is above the knee: {}", db(l));
        }
    }

    /// The ceiling is on *units*, before velocity — an operator at full output
    /// takes no boost from key scaling.
    #[test]
    fn key_scaling_cannot_boost_past_nominal() {
        // The ceiling is `min(127)` on units, i.e. nominal — which since 0325
        // sits 5.25 dB below full scale, that headroom being velocity's alone.
        let nominal = op_max_amp(99, 0, 0);
        assert!((op_max_amp(99, 40, 0) - nominal).abs() < 1e-6, "boost at full output");
        // A quieter operator has headroom to be boosted into: OL 60 sits at 88
        // units, 39 below the ceiling. +32 is an exact four-octave boost; +39
        // lands precisely on the ceiling and anything beyond stops there.
        let ratio = op_max_amp(60, 32, 0) / op_max_amp(60, 0, 0);
        assert!((ratio - 16.0).abs() < 1e-3, "unclamped boost: {ratio}");
        assert!((op_max_amp(60, 39, 0) - nominal).abs() < 1e-6, "boost reaches the ceiling");
        assert!((op_max_amp(60, 60, 0) - nominal).abs() < 1e-6, "and stops there");
    }

    /// Only velocity may exceed nominal; the floor clamp is the only bound
    /// after it.
    #[test]
    fn velocity_offset_is_the_only_route_above_nominal() {
        let nominal = op_max_amp(99, 0, 0);
        let boosted = op_max_amp(99, 0, vel_level_offset(127, 7));
        assert!(boosted > nominal, "velocity boost {boosted} <= nominal {nominal}");
        // Floor: a large negative offset bottoms out rather than going negative.
        assert!(op_max_amp(99, 0, -100_000) < 1e-4);
        assert!(op_max_amp(99, 0, -100_000) >= 0.0);
    }

    /// Exact against the reference across the whole input space — 128
    /// velocities x 8 sensitivities, integers, no tolerance. Catches the
    /// negative-shift rounding in particular.
    #[test]
    fn vel_level_offset_matches_hardware() {
        const DATA: [u8; 64] = [
            0, 70, 86, 97, 106, 114, 121, 126, 132, 138, 142, 148, 152, 156, 160, 163, 166, 170,
            173, 174, 178, 181, 184, 186, 189, 190, 194, 196, 198, 200, 202, 205, 206, 209, 211,
            214, 216, 218, 220, 222, 224, 225, 227, 229, 230, 232, 233, 235, 237, 238, 240, 241,
            242, 243, 244, 246, 246, 248, 249, 250, 251, 252, 253, 254,
        ];
        for vel in 0..=127u8 {
            for sens in 0..=7u8 {
                let vv = DATA[(vel >> 1) as usize] as i32 - 239;
                let want = ((sens as i32 * vv + 7) >> 3) << 4;
                assert_eq!(vel_level_offset(vel, sens), want, "vel {vel}, sens {sens}");
            }
        }
    }

    /// Sensitivity 0 is exactly velocity-independent.
    #[test]
    fn zero_sensitivity_ignores_velocity() {
        for vel in 0..=127u8 {
            assert_eq!(vel_level_offset(vel, 0), 0, "vel {vel} at sens 0");
        }
    }

    /// Nominal is 1.0 — unchanged, so every level-dependent stage downstream
    /// (filter drive, dynamics saturator) keeps its operating point. Velocity
    /// is what reaches above, and only on a hard strike.
    #[test]
    fn nominal_is_unity_and_velocity_reaches_above_it() {
        assert!((op_max_amp(99, 0, 0) - 1.0).abs() < 1e-6, "nominal is unity");
        let hardest = op_max_amp(99, 0, vel_level_offset(127, 7));
        assert!((hardest - MAX_ATTAINABLE_AMP).abs() < 1e-5, "hardest strike {hardest}");
        assert!((20.0 * hardest.log10() - 5.25).abs() < 0.05, "boost is 5.25 dB");
    }

    /// `MAX_ATTAINABLE_AMP` is the true supremum: nothing reachable through any
    /// combination of level, key scaling and velocity exceeds it. This is the
    /// bound the engine's level-mod projection clamps against, and the ramp
    /// between two in-range endpoints staying in range is what lets the
    /// per-sample lane loop skip a clamp of its own.
    #[test]
    fn max_attainable_amp_tracks_the_ladder() {
        let want = frac_to_amp(FULL_SCALE_FRAC + vel_level_offset(127, 7));
        assert!(
            (MAX_ATTAINABLE_AMP - want).abs() < 1e-6,
            "constant {MAX_ATTAINABLE_AMP} has drifted from the ladder's {want}"
        );
        for level in [1u8, 20, 58, 80, 99] {
            for ks in [-60i32, 0, 40, 99] {
                for sens in 0..=7u8 {
                    for vel in [0u8, 32, 64, 100, 127] {
                        let a = op_max_amp(level, ks, vel_level_offset(vel, sens));
                        assert!(
                            a <= MAX_ATTAINABLE_AMP + 1e-6,
                            "OL {level} ks {ks} sens {sens} vel {vel} → {a}"
                        );
                    }
                }
            }
        }
    }
}
