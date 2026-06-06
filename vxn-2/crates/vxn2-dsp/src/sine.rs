//! Sine reader candidates for the VXN2 operator core.
//!
//! Q32 fixed-point phase: full u32 = one cycle, wraparound is free via integer
//! add. Three readers share the domain; the operator core uses
//! [`scalar::fast_sine_q32`] / [`neon::fast_sine_q32_x4`] by default. The
//! lookup variant exists for the two-tier escape hatch noted in the README
//! (solo carriers where THD matters).
//!
//! - [`scalar::fast_sine_q32`] — Bhaskara I + Moser polynomial. Branch-free,
//!   pure ALU. Max abs err ≈ 0.001 vs `f64::sin`. THD ≈ -59 dB.
//! - [`scalar::lookup_sine_q32`] — 1024-entry table, linear interp. Max abs
//!   err ≈ 1e-6. THD ≈ -133 dB. Scalar gather → poor SIMD.
//! - [`neon::fast_sine_q32_x4`] — 4-lane NEON port of the polynomial.

use std::sync::LazyLock;

pub const TABLE_LEN: usize = 1024;
pub const TABLE_MASK: usize = TABLE_LEN - 1;

pub static SINE_TABLE: LazyLock<[f32; TABLE_LEN]> = LazyLock::new(|| {
    let mut t = [0.0f32; TABLE_LEN];
    for (i, slot) in t.iter_mut().enumerate() {
        *slot = (i as f32 / TABLE_LEN as f32 * std::f32::consts::TAU).sin();
    }
    t
});

pub mod scalar {
    use super::*;

    /// Bhaskara+Moser polynomial sine. Q32 phase in, f32 out.
    #[inline(always)]
    pub fn fast_sine_q32(phase: u32) -> f32 {
        let p = phase as f32 * (1.0 / 4_294_967_296.0);
        let x1 = p - 0.5;
        let x2 = x1 * 16.0 * (x1.abs() - 0.5);
        x2 + 0.225 * x2 * (x2.abs() - 1.0)
    }

    /// 1024-entry table lookup with linear interpolation.
    #[inline(always)]
    pub fn lookup_sine_q32(phase: u32, table: &[f32; TABLE_LEN]) -> f32 {
        let index_whole = (phase >> 22) as usize;
        let index_frac = (phase & 0x003F_FFFF) as f32 * (1.0 / 4_194_304.0);
        let a = table[index_whole];
        let b = table[(index_whole + 1) & TABLE_MASK];
        a + (b - a) * index_frac
    }
}

#[cfg(target_arch = "aarch64")]
pub mod neon {
    use std::arch::aarch64::*;

    /// 4-lane Bhaskara+Moser. Input is a Q32 phase per lane.
    #[inline]
    #[target_feature(enable = "neon")]
    pub unsafe fn fast_sine_q32_x4(phase: uint32x4_t) -> float32x4_t {
        let scale = vdupq_n_f32(1.0 / 4_294_967_296.0);
        let p = vmulq_f32(vcvtq_f32_u32(phase), scale);

        let half = vdupq_n_f32(0.5);
        let sixteen = vdupq_n_f32(16.0);
        let moser = vdupq_n_f32(0.225);
        let one = vdupq_n_f32(1.0);

        let x1 = vsubq_f32(p, half);
        let inner = vsubq_f32(vabsq_f32(x1), half);
        let x2 = vmulq_f32(vmulq_f32(x1, sixteen), inner);
        let outer = vsubq_f32(vabsq_f32(x2), one);
        vfmaq_f32(x2, vmulq_f32(x2, moser), outer)
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
    fn scalar_fast_sine_landmarks() {
        for (phase, expected) in [
            (0u32, 0.0f32),
            (0x4000_0000, 1.0),
            (0x8000_0000, 0.0),
            (0xC000_0000, -1.0),
        ] {
            let got = scalar::fast_sine_q32(phase);
            assert!(
                (got - expected).abs() < 2e-3,
                "fast_sine_q32({phase:#x}) = {got}, want ≈ {expected}"
            );
        }
    }

    #[test]
    fn scalar_lookup_sine_landmarks() {
        let t = &*SINE_TABLE;
        for (phase, expected) in [
            (0u32, 0.0f32),
            (0x4000_0000, 1.0),
            (0x8000_0000, 0.0),
            (0xC000_0000, -1.0),
        ] {
            let got = scalar::lookup_sine_q32(phase, t);
            assert!(
                (got - expected).abs() < 1e-3,
                "lookup_sine_q32({phase:#x}) = {got}, want ≈ {expected}"
            );
        }
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

    #[cfg(target_arch = "aarch64")]
    #[test]
    fn neon_matches_scalar() {
        use std::arch::aarch64::*;
        let phases = [0u32, 0x4000_0000, 0x8000_0000, 0xC000_0000];
        let p = unsafe { vld1q_u32(phases.as_ptr()) };
        let v = unsafe { neon::fast_sine_q32_x4(p) };
        let mut out = [0.0f32; 4];
        unsafe { vst1q_f32(out.as_mut_ptr(), v) };
        for (i, &phase) in phases.iter().enumerate() {
            let s = scalar::fast_sine_q32(phase);
            assert!(
                (out[i] - s).abs() < 1e-6,
                "lane {i}: neon {} vs scalar {s}",
                out[i]
            );
        }
    }
}
