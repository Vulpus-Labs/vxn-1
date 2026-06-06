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

/// Amp sensitivity (0..3). Coefficient applied to incoming LFO depth when the
/// matrix routes LFO→op level. The matrix multiplies its `depth` against this
/// receive coefficient. Approximate DX7 amp-mod sens table.
pub const AMP_SENS_TABLE: [f32; 4] = [0.0, 0.4, 0.7, 1.0];

#[inline]
pub fn amp_sens_coef(amp_sens: u8) -> f32 {
    AMP_SENS_TABLE[amp_sens.min(3) as usize]
}

/// Per-op feedback (0..7). Maps to a multiplier applied to the 2-sample-
/// averaged feedback signal before it's mixed into the phase-modulation input.
/// Below ~1.0 = warm saw; above ~1.0 heads toward noise. DX7's table tops out
/// around enough to make a sawtooth from a single op.
pub const FB_SCALE_TABLE: [f32; 8] = [
    0.0, 0.075, 0.150, 0.300, 0.600, 1.200, 2.000, 3.000,
];

#[inline]
pub fn fb_scale(feedback: u8) -> f32 {
    FB_SCALE_TABLE[feedback.min(7) as usize]
}

/// Detune (-7..+7). DX7's detune step is roughly 1.7 cents at A4, slightly
/// pitch-dependent in the original ROM. We use a flat 1.7 cents/step — close
/// enough; the detune knob is about thickening, not tuning.
#[inline]
pub fn detune_cents(detune: i8) -> f32 {
    let d = detune.clamp(-7, 7) as f32;
    d * 1.7
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
        for i in 0..8u8 {
            let v = fb_scale(i);
            assert!(v > prev, "fb_scale({i}) = {v} ≤ {prev}");
            prev = v;
        }
    }

    #[test]
    fn detune_zero_centered() {
        assert!((detune_cents(0)).abs() < 1e-6);
        assert!((detune_cents(7) + detune_cents(-7)).abs() < 1e-6);
    }
}
