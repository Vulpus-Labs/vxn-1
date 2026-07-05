//! Sine reader for the VXN2 operator core.
//!
//! Q32 fixed-point phase: full u32 = one cycle, wraparound is free via integer
//! add. The operator core uses [`scalar::fast_sine_q32`] everywhere; LLVM
//! auto-vectorises it across the 8-lane SoA loop, so there is no hand-written
//! NEON path and no table variant.
//!
//! - [`scalar::fast_sine_q32`] — Bhaskara I + Moser polynomial. Branch-free,
//!   pure ALU. Max abs err ≈ 0.001 vs `f64::sin`. THD ≈ -59 dB.

pub mod scalar {
    /// Bhaskara+Moser polynomial sine of a phase fraction `p ∈ [0, 1)`
    /// (`p = phase / cycle`). Branch-free, pure ALU. The float-phase form of
    /// the operator-core sine; [`fast_sine_q32`] is the Q32 wrapper and the
    /// reverb LFO ([`crate::reverb`]) calls this directly (ticket 0071 — was a
    /// third hand-inlined copy of the polynomial).
    #[inline(always)]
    pub fn fast_sine_01(p: f32) -> f32 {
        let x1 = p - 0.5;
        let x2 = x1 * 16.0 * (x1.abs() - 0.5);
        x2 + 0.225 * x2 * (x2.abs() - 1.0)
    }

    /// Bhaskara+Moser polynomial sine. Q32 phase in, f32 out.
    #[inline(always)]
    pub fn fast_sine_q32(phase: u32) -> f32 {
        fast_sine_01(phase as f32 * (1.0 / 4_294_967_296.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sin_truth_q32(phase: u32) -> f32 {
        let p = phase as f64 / 4_294_967_296.0;
        (p * std::f64::consts::TAU).sin() as f32
    }

    #[test]
    fn fast_sine_accuracy() {
        let steps = 100_000u32;
        let mut max_err = 0.0f32;
        for i in 0..steps {
            let phase = ((i as u64 * (1u64 << 32) / steps as u64) as u32) & u32::MAX;
            let got = scalar::fast_sine_q32(phase);
            let truth = sin_truth_q32(phase);
            let err = (got - truth).abs();
            if err > max_err {
                max_err = err;
            }
        }
        assert!(max_err < 2e-3, "max abs err {max_err} exceeds 2e-3");
    }
}
