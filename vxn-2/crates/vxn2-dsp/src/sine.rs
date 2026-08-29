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
    /// Re-export: the float-phase form of the operator-core sine moved to
    /// `vxn-core-utils` in ticket 0230, so the shared FDN reverb's LFO and this
    /// crate's operator core read one definition instead of two copies.
    pub use vxn_core_utils::math::fast_sine_01;

    /// Bhaskara+Moser polynomial sine. Q32 phase in, f32 out.
    #[inline(always)]
    pub fn fast_sine_q32(phase: u32) -> f32 {
        fast_sine_01(vxn_core_utils::math::q32_to_unit(phase))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sin_truth_q32(phase: u32) -> f32 {
        let p = vxn_core_utils::math::q32_to_unit_f64(phase);
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
