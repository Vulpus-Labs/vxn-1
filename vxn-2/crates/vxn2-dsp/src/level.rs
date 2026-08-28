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

/// Accumulator value denoting full scale (amplitude 1.0).
///
/// Ticket 0325 moves this to the maximum *attainable* level so that velocity's
/// boost above nominal has headroom; until then full scale is nominal, and a
/// boost would be swallowed by the `[0, 1]` clamp in the engine's level-mod
/// projection.
pub const FULL_SCALE_FRAC: i32 = MAX_UNITS * FRAC_PER_UNIT;

/// Output-level curve below the knee. Above `OL 20` the hardware is linear
/// (`28 + OL`); below it this table compresses, so quiet operators fall away
/// faster than a uniform 0.75 dB/step would take them.
const LEVELLUT: [i32; 20] = [
    0, 5, 9, 13, 17, 20, 23, 25, 27, 29, 31, 33, 35, 37, 39, 41, 42, 43, 45, 46,
];

/// Operator output level (0..99) → level units (0..127).
///
/// Above the knee this is exactly [`crate::eg::level_to_amp`]'s curve — `28 + OL`
/// against a full scale of 127 is `2^((OL−99)/8)`, which is why ADR 0007's
/// 0.75 dB/step matched the hardware without per-patch fudging. Below `OL 20`
/// they diverge: the table compresses (−95.6 dB at OL 0 against ADR 0007's
/// −74.5 dB), so a very quiet operator is quieter here than it was.
#[inline]
pub fn scale_outlevel(level: u8) -> i32 {
    let l = level.min(99) as i32;
    if l >= 20 { 28 + l } else { LEVELLUT[l as usize] }
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

    /// Above the knee the accumulator reproduces ADR 0007's curve exactly, so
    /// the calibration that bank is voiced against is preserved.
    #[test]
    fn agrees_with_adr_0007_above_the_knee() {
        for l in 20..=99u8 {
            let via_units = op_max_amp(l, 0, 0);
            let via_curve = level_to_amp(l, EgCurve::Exp);
            let ratio_db = 20.0 * (via_units / via_curve).log10();
            assert!(ratio_db.abs() < 1e-3, "OL {l}: {ratio_db:.4} dB apart");
        }
    }

    /// Below the knee it deliberately does not: the table compresses. Pinned so
    /// the divergence is a recorded decision rather than a surprise.
    #[test]
    fn diverges_below_the_knee_by_the_table() {
        let db = |l: u8| 20.0 * (op_max_amp(l, 0, 0) / level_to_amp(l, EgCurve::Exp)).log10();
        assert!((db(19) - -0.8).abs() < 0.15, "OL 19: {}", db(19));
        assert!((db(12) - -3.8).abs() < 0.15, "OL 12: {}", db(12));
        assert!((db(4) - -11.3).abs() < 0.15, "OL 4: {}", db(4));
        assert_eq!(op_max_amp(0, 0, 0), 0.0, "OL 0 is hard silence");
    }

    /// The ceiling is on *units*, before velocity — an operator at full output
    /// takes no boost from key scaling.
    #[test]
    fn key_scaling_cannot_boost_past_full_scale() {
        assert!((op_max_amp(99, 0, 0) - 1.0).abs() < 1e-6);
        assert!((op_max_amp(99, 40, 0) - 1.0).abs() < 1e-6, "boost at full output");
        // A quieter operator has headroom to be boosted into: OL 60 sits at 88
        // units, 39 below the ceiling. +32 is an exact four-octave boost; +39
        // lands precisely on the ceiling and anything beyond stops there.
        let ratio = op_max_amp(60, 32, 0) / op_max_amp(60, 0, 0);
        assert!((ratio - 16.0).abs() < 1e-3, "unclamped boost: {ratio}");
        assert!((op_max_amp(60, 39, 0) - 1.0).abs() < 1e-6, "boost reaches the ceiling");
        assert!((op_max_amp(60, 60, 0) - 1.0).abs() < 1e-6, "and stops there");
    }

    /// Only velocity may exceed full scale; the floor clamp is the only bound
    /// after it. (Nothing supplies a positive offset until 0324.)
    #[test]
    fn velocity_offset_is_the_only_route_above_full_scale() {
        let boosted = op_max_amp(99, 0, 7 * FRAC_PER_UNIT);
        assert!(boosted > 1.0, "velocity boost reaches {boosted}, want > 1");
        // Floor: a large negative offset bottoms out rather than going negative.
        assert!(op_max_amp(99, 0, -100_000) < 1e-4);
        assert!(op_max_amp(99, 0, -100_000) >= 0.0);
    }
}
