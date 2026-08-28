//! Key scaling — both level and rate.
//!
//! ## Level scaling
//!
//! A port of the reference hardware's `ScaleLevel`/`ScaleCurve`, not an
//! approximation of it. The hardware computes a **level offset** — in the same
//! ~0.75 dB units as the operator output level — and *adds* it to that level
//! before the level→amplitude conversion. We return the offset already folded
//! into an amplitude multiplier (adding in dB ≡ multiplying in amplitude), so
//! the call sites keep their `level_norm × ks × vel` shape.
//!
//! Three details are load-bearing, and all three were wrong in the closed-form
//! approximation this replaces (which produced −0.1…−3.8 dB where the hardware
//! produces +17…−49, i.e. a control that did almost nothing):
//!
//! * **The offset is logarithmic, not a linear amplitude fade.** Depth is dB
//!   per semitone.
//! * **The slope is fixed per depth**, independent of break-point position.
//!   The old code normalised the ramp to the far edge of the keyboard, so the
//!   same depth meant a different slope for every break point.
//! * **The pivot sits 4 semitones below the break point**, with a ~±2-semitone
//!   dead zone from the hardware's 3-semitone grouping. (Hardware works in the
//!   raw 0..99 break-point parameter and offsets by 17; we store the break
//!   point as a MIDI note, `raw + 21`, hence `note − bp + 4`.)
//!
//! Level scaling can *boost* as well as cut, and the accumulator clamps the
//! summed level at its ceiling before velocity — so an operator already at full
//! output gets no boost at all.
//!
//! ## Rate scaling
//!
//! Rate scaling speeds up all four EG rates as note pitch rises. A single
//! `ks_rate` (0..7) parameter applies uniformly.

/// Per-group offsets for the **exponential** curves, from the reference
/// hardware. Indexed by the 3-semitone group number (saturating at the last
/// entry, i.e. 96 semitones out); the linear curves use the group number
/// directly instead.
const EXP_SCALE_DATA: [u8; 33] = [
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 11, 14, 16, 19, 23, 27, 33, 39, 47, 56, 66, 80, 94, 110, 126,
    142, 158, 174, 190, 206, 222, 238, 250,
];

/// Semitones the scaling pivot sits below the stored (MIDI) break point.
/// Hardware: `offset = note − raw_bp − 17`, and `raw_bp = bp_midi − 21`.
const BP_PIVOT_OFFSET: i32 = 4;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum KsCurve {
    NegLin = 0,
    PosLin = 1,
    NegExp = 2,
    PosExp = 3,
}

impl Default for KsCurve {
    fn default() -> Self {
        KsCurve::NegLin
    }
}

/// One side's level offset, in level units (~0.75 dB each). `group` is the
/// 3-semitone group number away from the pivot; `depth` is 0..99. Mirrors the
/// hardware's `ScaleCurve`, including its integer truncation — the steps are
/// audible at low depths and smoothing them would not be more faithful.
fn scale_curve(group: u32, depth: u8, curve: KsCurve) -> i32 {
    let depth = depth.min(99) as u32;
    let scale = match curve {
        // Linear: the group number itself is the ramp.
        KsCurve::NegLin | KsCurve::PosLin => (group * depth * 329) >> 12,
        // Exponential: a tabulated ramp, then the same depth scaling.
        KsCurve::NegExp | KsCurve::PosExp => {
            let raw = EXP_SCALE_DATA[(group as usize).min(EXP_SCALE_DATA.len() - 1)] as u32;
            (raw * depth * 329) >> 15
        }
    };
    match curve {
        KsCurve::NegLin | KsCurve::NegExp => -(scale as i32),
        KsCurve::PosLin | KsCurve::PosExp => scale as i32,
    }
}

/// Keyboard-level offset for a note, in level units (~0.75 dB each). Positive
/// boosts, negative cuts, zero at the pivot. Summed into the operator's level
/// accumulator by [`crate::level::op_max_amp`].
pub fn ks_level_offset(
    key: u8,
    break_pt: u8,
    l_depth: u8,
    l_curve: KsCurve,
    r_depth: u8,
    r_curve: KsCurve,
) -> i32 {
    let offset = key.min(127) as i32 - break_pt.min(127) as i32 + BP_PIVOT_OFFSET;
    if offset >= 0 {
        scale_curve(((offset + 1) / 3) as u32, r_depth, r_curve)
    } else {
        scale_curve(((-(offset - 1)) / 3) as u32, l_depth, l_curve)
    }
}

/// Rate-scaling multiplier on EG rates. `1.0` at MIDI A3 (note 57). Above
/// that, rates increase; below, they decrease. `ks_rate` (0..7) sets the
/// strength. At ks_rate=7 the rates roughly double per 2 octaves of upward
/// motion.
pub fn ks_rate_mult(key: u8, ks_rate: u8) -> f32 {
    let key = key.min(127) as f32;
    let octaves_from_a3 = (key - 57.0) / 12.0;
    let strength = (ks_rate.min(7) as f32) / 7.0;
    // 2^(strength * octaves * 0.5) — at strength=1, +2 octaves doubles rates.
    2_f32.powf(strength * octaves_from_a3 * 0.5)
}

#[cfg(test)]
mod tests {
    use super::*;
    use KsCurve::*;

    /// Level offsets against the reference implementation, computed from its
    /// `ScaleLevel`/`ScaleCurve` for these exact inputs. Integers, so this is an
    /// exact match, not a tolerance — any drift in the grouping, the pivot
    /// offset, the 329/shift constants or the exp table trips it.
    #[test]
    fn level_offset_matches_hardware_reference() {
        // (key, bp, l_depth, l_curve, r_depth, r_curve) -> level units
        let cases: [(u8, u8, u8, KsCurve, u8, KsCurve, i32); 8] = [
            (60, 60, 0, NegLin, 0, NegLin, 0),
            (56, 60, 99, PosLin, 99, NegLin, 0), // inside the pivot dead zone
            (96, 60, 0, PosLin, 99, NegLin, -103),
            (96, 60, 0, PosLin, 99, NegExp, -18),
            (24, 60, 99, PosLin, 0, NegLin, 87), // left side boosts
            (96, 21, 0, PosLin, 74, NegLin, -154),
            (96, 68, 0, PosLin, 74, NegLin, -65),
            (84, 60, 0, PosLin, 30, NegExp, -2),
        ];
        for (key, bp, ld, lc, rd, rc, want) in cases {
            let got = ks_level_offset(key, bp, ld, lc, rd, rc);
            assert_eq!(got, want, "ks_level_offset(key {key}, bp {bp}, r_depth {rd})");
        }
    }

    /// The property the old closed form got wrong: depth sets a fixed slope in
    /// dB per semitone, so the same distance from the pivot gives the same
    /// offset wherever the break point sits. The old code normalised its ramp
    /// to the far edge of the keyboard, making the slope a function of the
    /// break point.
    #[test]
    fn slope_is_independent_of_break_point() {
        let at = |bp: u8| ks_level_offset(bp + 24, bp, 0, PosLin, 50, NegLin);
        assert_eq!(at(40), -36);
        assert_eq!(at(60), -36);
        assert_eq!(at(80), -36);
    }

    #[test]
    fn zero_offset_at_the_pivot() {
        // Full depth both sides, but at the pivot the offset is 0 → no change.
        assert_eq!(ks_level_offset(56, 60, 99, PosExp, 99, NegLin), 0);
    }

    #[test]
    fn neg_curves_cut_and_pos_curves_boost() {
        // 3 octaves above the break point, full depth.
        assert!(ks_level_offset(96, 60, 0, PosLin, 99, NegLin) < -60);
        assert!(ks_level_offset(96, 60, 0, PosLin, 99, PosLin) > 60);
        // 3 octaves below, full depth.
        assert!(ks_level_offset(24, 60, 99, PosLin, 0, NegLin) > 60);
        assert!(ks_level_offset(24, 60, 99, NegLin, 0, NegLin) < -60);
    }

    /// Exp ramps far more gently than lin near the break point — the tabulated
    /// curve only accelerates in the top half of its range.
    #[test]
    fn exp_bends_below_lin_across_the_interior() {
        for note in [66u8, 72, 78, 84, 90, 96] {
            let lin = ks_level_offset(note, 60, 0, PosLin, 99, NegLin);
            let exp = ks_level_offset(note, 60, 0, PosLin, 99, NegExp);
            assert!(exp > lin, "note {note}: exp {exp} should cut less than lin {lin}");
        }
    }

    /// A depth of 0 is a flat keyboard on either side, at any break point.
    #[test]
    fn zero_depth_is_flat() {
        for note in [0u8, 24, 60, 96, 127] {
            assert_eq!(ks_level_offset(note, 60, 0, NegLin, 0, NegExp), 0);
        }
    }

    #[test]
    fn rate_mult_unity_at_a3() {
        assert!((ks_rate_mult(57, 7) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn rate_mult_doubles_two_octaves_up_at_full() {
        let m = ks_rate_mult(81, 7); // 81 = A3 + 24 semitones = 2 octaves.
        assert!((m - 2.0).abs() < 1e-3, "two-octave rate scale: {m}");
    }
}
