//! Key scaling — both level and rate.
//!
//! At the break point, scaling has no effect. Below the BP the `l_*`
//! parameters apply; above, the `r_*` parameters. Curve type sets shape (lin
//! vs exp) and sign (boost vs cut). Each side ramps from the break point to
//! the keyboard edge it faces (note 0 on the left, 127 on the right): full
//! depth lands exactly at the extreme note, so the ramp's reach scales with
//! break-point position. DX7's ROM uses tabulated dB offsets; we approximate
//! with a continuous closed-form so the result is smooth across the key range.
//!
//! Rate scaling speeds up all four EG rates as note pitch rises. A single
//! `ks_rate` (0..7) parameter applies uniformly — matches DX7 RKS.

/// Curvature of the exponential shape. The `exp` curve is a true normalised
/// exponential `(e^(K·t) − 1) / (e^K − 1)` mapping `t∈[0,1] → [0,1]`; the
/// endpoints are fixed (0 at the BP, 1 at the edge) and `K` only bends the
/// interior. Larger `K` = later, sharper onset. Kept in lockstep with the
/// UI port in `ks-graph.js` (`KS_EXP_K`).
pub const KS_EXP_K: f32 = 3.0;

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

/// Compute the keyboard-level multiplier for a given note. Returns a value
/// usually in `[0.0, ~2.0]` — `1.0` at the break point. Multiplied against
/// the per-op `level` to produce the cooked per-note amplitude scaler.
///
/// `depth` parameters are 0..99; `key` and `break_pt` are MIDI note numbers.
pub fn ks_level_mult(
    key: u8,
    break_pt: u8,
    l_depth: u8,
    l_curve: KsCurve,
    r_depth: u8,
    r_curve: KsCurve,
) -> f32 {
    let key = key.min(127) as i32;
    let bp = break_pt.min(127) as i32;
    let semitones = key - bp;
    // Pick the facing side and its reach to the keyboard edge: right side
    // spans BP..127, left side spans 0..BP. `t` is 0 at the BP and 1 at the
    // extreme note, so full depth lands on the edge rather than a fixed span.
    let (depth, curve, reach) = if semitones >= 0 {
        (r_depth, r_curve, 127 - bp)
    } else {
        (l_depth, l_curve, bp)
    };
    let t = if reach > 0 {
        (semitones.unsigned_abs() as f32 / reach as f32).min(1.0)
    } else {
        0.0
    };
    let shape = match curve {
        KsCurve::PosLin | KsCurve::NegLin => t,
        KsCurve::PosExp | KsCurve::NegExp => {
            // True normalised exponential: bends late, sharp near the edge.
            ((KS_EXP_K * t).exp() - 1.0) / (KS_EXP_K.exp() - 1.0)
        }
    };
    let sign = match curve {
        KsCurve::PosLin | KsCurve::PosExp => 1.0,
        KsCurve::NegLin | KsCurve::NegExp => -1.0,
    };
    let depth_norm = (depth.min(99) as f32) / 99.0;
    let mult = 1.0 + sign * depth_norm * shape;
    mult.max(0.0)
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

    #[test]
    fn unity_at_break_point() {
        let m = ks_level_mult(60, 60, 99, KsCurve::PosExp, 99, KsCurve::NegLin);
        assert!((m - 1.0).abs() < 1e-6, "at BP, mult = {m}");
    }

    #[test]
    fn neg_lin_cuts_above_bp() {
        let m = ks_level_mult(96, 60, 0, KsCurve::PosLin, 99, KsCurve::NegLin);
        // Right reach = 127 - 60 = 67; t = 36/67 ≈ 0.537; lin, -depth →
        // 1 - 0.537 ≈ 0.463.
        assert!(m > 0.44 && m < 0.48, "neg lin above bp: {m}");
    }

    #[test]
    fn full_depth_at_keyboard_edge() {
        // Full depth is reached at the extreme note, not a fixed span.
        let m = ks_level_mult(127, 60, 0, KsCurve::PosLin, 99, KsCurve::PosExp);
        assert!(m > 1.99, "pos exp at top of keyboard: {m}");
        let m = ks_level_mult(0, 60, 99, KsCurve::NegLin, 0, KsCurve::PosLin);
        assert!(m < 0.01, "neg lin at bottom of keyboard: {m}");
    }

    #[test]
    fn exp_bends_below_lin_in_interior() {
        // Same key/depth: the exponential curve sits closer to unity than the
        // linear one until the shared edge endpoint.
        let lin = ks_level_mult(90, 60, 0, KsCurve::PosLin, 99, KsCurve::PosLin);
        let exp = ks_level_mult(90, 60, 0, KsCurve::PosLin, 99, KsCurve::PosExp);
        assert!(exp < lin, "exp {exp} should bend below lin {lin}");
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
