//! Small lookup tables that translate plain FM-operator parameter values to
//! runtime scalars. Values are not bit-exact but match the *shape* described in
//! PARAMETERS.md and ADR 0001 — fidelity target is "sounds like an FM
//! operator", not byte-exact reproduction.

/// Velocity sensitivity (0..7). Approximate vel-sens curve: at 0,
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

/// Layer-level feedback (continuous, `[0.0, 7.0]`) → the multiplier applied to
/// the 2-sample-averaged feedback signal before it is mixed into the
/// phase-modulation input.
///
/// The reference hardware shifts the averaged pair right by `8 − fb`, i.e. a
/// gain of `2^(fb − 8)`: 1/2 at fb 7, 1/4 at fb 6, down to 1/128 at fb 1, and
/// off at 0. Both engines put full-scale operator output at one cycle of phase,
/// so the ladders are directly comparable.
///
/// This is a closed form rather than the table it replaces, for two reasons.
/// The table topped out at 1.0 — exactly 2× the hardware at every step, which
/// also put the loop's maximum past the sawtooth edge into the chaotic region
/// where an operator EG releasing through the stability boundary collapses the
/// oscillation mode in a couple of samples (an unsmoothable note-off click).
/// And `feedback` is continuous, so interpolating *linearly* between entries of
/// a geometric ladder gave the wrong dB taper between whole steps.
///
/// Below fb 1 the gain fades linearly to silence, so that `fb = 0` is true off
/// rather than `2^-8`, and the approach to it stays continuous.
#[inline]
pub fn fb_scale(feedback: f32) -> f32 {
    let x = feedback.clamp(0.0, 7.0);
    if x <= 0.0 {
        return 0.0;
    }
    if x < 1.0 {
        // 2^-7 at fb 1, linearly down to 0 — the ladder's own bottom rung
        // scaled by the fractional part.
        return x * (1.0_f32 / 128.0);
    }
    (x - 8.0).exp2()
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

    /// Integer steps match the reference ladder `2^(fb − 8)` exactly: 1/2 at
    /// the top, halving per step. The table this replaced was 2× at every rung.
    #[test]
    fn fb_scale_integer_steps_match_hardware_ladder() {
        assert_eq!(fb_scale(0.0), 0.0);
        for fb in 1..8u32 {
            let want = 2_f32.powi(fb as i32 - 8);
            let got = fb_scale(fb as f32);
            assert!((got - want).abs() < 1e-7, "fb_scale({fb}) = {got}, want {want}");
        }
        assert!((fb_scale(7.0) - 0.5).abs() < 1e-7, "top of the ladder is 1/2, not 1");
    }

    /// Geometric, not linear, between whole steps — a half-step is 2^0.5 of the
    /// rung below, so the taper is even in dB across the control's range.
    #[test]
    fn fb_scale_is_geometric_between_steps() {
        for fb in [1.5_f32, 3.5, 6.5] {
            let want = (fb - 8.0).exp2();
            let got = fb_scale(fb);
            assert!((got - want).abs() < 1e-7, "fb_scale({fb}) = {got}, want {want}");
        }
        // Equal ratios for equal intervals, anywhere on the ladder.
        let r1 = fb_scale(4.5) / fb_scale(3.5);
        let r2 = fb_scale(6.5) / fb_scale(5.5);
        assert!((r1 - r2).abs() < 1e-5, "taper not even in dB: {r1} vs {r2}");
    }

    /// The sub-1.0 tail fades to true silence, continuously.
    #[test]
    fn fb_scale_fades_to_off_below_one() {
        assert_eq!(fb_scale(0.0), 0.0);
        assert!((fb_scale(0.5) - 1.0 / 256.0).abs() < 1e-7);
        // Continuous at the fb = 1 join.
        assert!((fb_scale(0.999) - fb_scale(1.0)).abs() < 1e-4);
    }

    #[test]
    fn fb_scale_clamps_out_of_range() {
        assert_eq!(fb_scale(-1.0), 0.0);
        assert!((fb_scale(99.0) - 0.5).abs() < 1e-7);
    }
}
