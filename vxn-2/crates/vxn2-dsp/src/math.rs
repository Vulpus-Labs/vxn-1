//! Fast math approximations. Lifted from VXN1 (`vxn-dsp/src/math.rs`), trimmed
//! to the single consumer VXN2 needs: the per-stage `tanh` of the OTA-C ladder
//! ([`crate::filter`]). VXN2's operators are pure sine, so this is the only
//! nonlinearity in the build. Error bounds and rationale are preserved verbatim.
//!
//! `vxn2-dsp` is deliberately dependency-free: we *copy* VXN1 primitives rather
//! than depend on the VXN1 crate (see `smoother.rs`).

/// Rational (Padé degree-5/6) approximation to `tanh`, saturating to ±1 for
/// `|x| ≥ 2.5`. Exact at 0, monotone, RMS error < 0.05 over [−3, 3].
///
/// The ±2.5 hard-clamp branches are hot-path-sensitive (VXN1's
/// `tanh-branchless-only` lesson — branch-free variants matter in the poly hot
/// loop, and swapping the clamp regresses); keep the branch structure as-is and
/// re-measure rather than refactoring.
#[inline(always)]
pub fn fast_tanh(x: f32) -> f32 {
    if x >= 2.5 {
        return 1.0;
    }
    if x <= -2.5 {
        return -1.0;
    }
    let x2 = x * x;
    let x4 = x2 * x2;
    let x6 = x4 * x2;
    x * (10395.0 + 1260.0 * x2 + 21.0 * x4) / (10395.0 + 4725.0 * x2 + 210.0 * x4 + 4.0 * x6)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tanh_key_points() {
        assert_eq!(fast_tanh(0.0), 0.0);
        assert!((fast_tanh(10.0) - 1.0).abs() < 1e-6);
        assert!((fast_tanh(-10.0) + 1.0).abs() < 1e-6);
    }

    #[test]
    fn tanh_is_odd() {
        let mut x = -3.0f32;
        while x <= 3.0 {
            assert!((fast_tanh(x) + fast_tanh(-x)).abs() < 1e-6, "odd at {x}");
            x += 0.01;
        }
    }

    #[test]
    fn tanh_monotone_and_bounded() {
        let mut prev = fast_tanh(-3.0);
        let mut x = -3.0f32;
        while x <= 3.0 {
            let y = fast_tanh(x);
            assert!(y >= prev - 1e-7, "non-monotone at {x}");
            assert!((-1.0..=1.0).contains(&y), "out of range at {x}: {y}");
            prev = y;
            x += 0.01;
        }
    }
}
