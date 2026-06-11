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
/// the phase-modulation input. DX7 feedback is shift-based — exactly ×2 per
/// step — so the table is a pure doubling ladder topping out at ~1.0, the
/// sawtooth edge of the feedback loop's stable region (ticket 0079).
///
/// The previous table extended to 3.0: past ~1.0 the loop runs chaotic, and
/// an op EG releasing through the stability boundary collapses the
/// oscillation mode within a couple of samples — an unsmoothable click on
/// every note-off (heard on the default E.PIANO, whose ROM FB=6 mapped to
/// 2.0). DX7 ROM voices now land on DX7-equivalent loop gains verbatim;
/// intermediate values linearly interpolate.
pub const FB_SCALE_TABLE: [f32; 8] = [
    0.0,
    1.0 / 64.0,
    1.0 / 32.0,
    1.0 / 16.0,
    1.0 / 8.0,
    1.0 / 4.0,
    1.0 / 2.0,
    1.0,
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
        // Halfway between step 3 (1/16) and step 4 (1/8) → 3/32.
        let v = fb_scale(3.5);
        assert!((v - 3.0 / 32.0).abs() < 1e-5, "fb_scale(3.5) = {v}");
    }

    #[test]
    fn fb_scale_clamps_out_of_range() {
        assert_eq!(fb_scale(-1.0), 0.0);
        assert_eq!(fb_scale(99.0), FB_SCALE_TABLE[7]);
    }
}
