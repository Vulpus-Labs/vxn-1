//! Key scaling — both level and rate.
//!
//! At the break point, scaling has no effect. Below the BP the `l_*`
//! parameters apply; above, the `r_*` parameters. Curve type sets shape (lin
//! vs exp) and sign (boost vs cut). DX7's ROM uses tabulated dB offsets per
//! octave; we approximate with a continuous closed-form so the result is
//! smooth across the key range.
//!
//! Rate scaling speeds up all four EG rates as note pitch rises. A single
//! `ks_rate` (0..7) parameter applies uniformly — matches DX7 RKS.

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
    let semitones = (key - bp) as f32;
    let (depth, curve) = if semitones >= 0.0 {
        (r_depth, r_curve)
    } else {
        (l_depth, l_curve)
    };

    // Distance in octaves, clamped to 4 octaves (full DX7 range).
    let d = (semitones.abs() / 12.0).min(4.0);
    let shape = match curve {
        KsCurve::PosLin | KsCurve::NegLin => d / 4.0,
        KsCurve::PosExp | KsCurve::NegExp => {
            // Exponential: more aggressive at the extremes.
            let t = d / 4.0;
            t * t
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
        // 96 - 60 = 36 semitones = 3 octaves; t = 3/4 = 0.75; -0.75 → 0.25 mult.
        assert!(m < 0.30 && m > 0.20, "neg lin 3 oct above: {m}");
    }

    #[test]
    fn pos_exp_boosts_above_bp() {
        let m = ks_level_mult(108, 60, 0, KsCurve::PosLin, 99, KsCurve::PosExp);
        // 4 octaves above, exp curve at full depth → 1 + 1*1 = 2.
        assert!(m > 1.9, "pos exp 4 oct above: {m}");
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
