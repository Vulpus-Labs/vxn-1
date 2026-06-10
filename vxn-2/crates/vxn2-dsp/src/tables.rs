//! Small lookup tables that translate plain DX7-style parameter values to
//! runtime scalars. Approximate the DX7 ROM curves; values are not bit-exact
//! but match the *shape* the synth manuals describe (see PARAMETERS.md and
//! ADR 0001 — fidelity target is "sounds like an FM operator", not byte-exact
//! DX7 reproduction).

/// Velocity sensitivity (0..7). Approximates the DX7 vel-sens curve: at 0,
/// `level` is independent of velocity (1.0 always). At 7, a velocity of 1
/// yields ~0 amplitude and 127 yields full. Intermediate `vs` interpolates
/// linearly between the two extremes.
#[inline]
pub fn vel_factor(vs: u8, velocity: u8) -> f32 {
    let vs = vs.min(7) as f32 / 7.0;
    let v = velocity.min(127) as f32 / 127.0;
    let v_curve = v * v; // squared = perceptual-ish.
    1.0 - vs * (1.0 - v_curve)
}

/// Layer-level feedback (continuous, `[0.0, 7.0]`). Maps to a multiplier
/// applied to the 2-sample-averaged feedback signal before it's mixed into
/// the phase-modulation input. Below ~1.0 = warm saw; above ~1.0 heads
/// toward noise. DX7's discrete steps land on the integer positions so
/// existing presets sound identical; intermediate values linearly interpolate
/// the quasi-log curve.
pub const FB_SCALE_TABLE: [f32; 8] = [
    0.0, 0.075, 0.150, 0.300, 0.600, 1.200, 2.000, 3.000,
];

#[inline]
pub fn fb_scale(feedback: f32) -> f32 {
    let x = feedback.clamp(0.0, 7.0);
    let lo = x.floor() as usize;
    if lo >= 7 {
        return FB_SCALE_TABLE[7];
    }
    let frac = x - lo as f32;
    FB_SCALE_TABLE[lo] + (FB_SCALE_TABLE[lo + 1] - FB_SCALE_TABLE[lo]) * frac
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vel_factor_endpoints() {
        // vs = 0: velocity-independent.
        for v in [1u8, 64, 127] {
            assert!((vel_factor(0, v) - 1.0).abs() < 1e-6);
        }
        // vs = 7: full attenuation at v=0, no attenuation at v=127.
        assert!(vel_factor(7, 0) < 0.05);
        assert!((vel_factor(7, 127) - 1.0).abs() < 1e-6);
        // monotonic in velocity for non-zero vs.
        assert!(vel_factor(7, 1) < vel_factor(7, 64));
        assert!(vel_factor(7, 64) < vel_factor(7, 127));
    }

    #[test]
    fn fb_scale_monotone() {
        let mut prev = -1.0;
        for i in 0..8u32 {
            let v = fb_scale(i as f32);
            assert!(v > prev, "fb_scale({i}) = {v} ≤ {prev}");
            prev = v;
        }
    }

    #[test]
    fn fb_scale_integer_inputs_match_table() {
        for i in 0..8 {
            assert!((fb_scale(i as f32) - FB_SCALE_TABLE[i]).abs() < 1e-7);
        }
    }

    #[test]
    fn fb_scale_interpolates_between_steps() {
        // Halfway between step 3 (0.300) and step 4 (0.600) → 0.450.
        let v = fb_scale(3.5);
        assert!((v - 0.45).abs() < 1e-5, "fb_scale(3.5) = {v}");
    }

    #[test]
    fn fb_scale_clamps_out_of_range() {
        assert_eq!(fb_scale(-1.0), 0.0);
        assert_eq!(fb_scale(99.0), FB_SCALE_TABLE[7]);
    }
}
