//! Shared scalar math approximations.
//!
//! The branched-scalar `fast_tanh` was byte-identical in `vxn-dsp` and
//! `vxn2-dsp`; E027/0118 folded both copies here. The branchless **poly-lane**
//! tanh in `vxn-dsp::poly::oscillator` is deliberately NOT merged in — its
//! `clamp` form vectorises in the per-lane hot loop where this early-return
//! form would not (memory `vxn1-tanh-branchless-only`).

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
            assert!(y >= prev - 1e-6, "not monotone at {x}");
            assert!((-1.0..=1.0).contains(&y), "out of range at {x}: {y}");
            prev = y;
            x += 0.01;
        }
    }
}

// ---------------------------------------------------------------------------
// Q32 phase convention (ticket 0224).
//
// A `u32` holds one full cycle of phase: 0 → 0.0, 2^32 → 1.0, and the wrap is
// the type's own `wrapping_add`. vxn-2, and any future synth that wants exact
// phase arithmetic, use it; vxn-1 keeps its normalised-f32 convention and is
// deliberately not forced onto this.
//
// The constants were spelled out at seven sites across `op.rs`, `lfo.rs`,
// `sine.rs` and `stack.rs`, at two precisions. Both precisions are kept, and
// each is used exactly where its old literal was — the f32 reciprocal-multiply
// and the f64 divide are NOT interchangeable, and swapping either would move
// the render hash.
// ---------------------------------------------------------------------------

/// Q32 phase units per full cycle (2^32), as `f32`. Multiply a unit-cycle value
/// by this to get Q32 phase.
pub const Q32_PER_CYCLE: f32 = 4_294_967_296.0;

/// Q32 phase units per full cycle (2^32), as `f64`. The wider form, for cook-time
/// arithmetic that must not lose bits before truncating to `u32`.
pub const Q32_PER_CYCLE_F64: f64 = 4_294_967_296.0;

/// Reciprocal of [`Q32_PER_CYCLE`], as `f32`. A compile-time constant so the hot
/// path multiplies rather than divides — the form every audio-rate site used.
pub const INV_Q32_PER_CYCLE: f32 = 1.0 / 4_294_967_296.0;

/// Q32 phase → unit cycle `[0, 1)` as `f32`. The audio-rate conversion: a
/// reciprocal multiply, not a divide.
#[inline]
pub fn q32_to_unit(phase: u32) -> f32 {
    phase as f32 * INV_Q32_PER_CYCLE
}

/// Q32 phase → unit cycle `[0, 1)` as `f64`. The reference-precision form, used
/// by test oracles rather than the hot path; a true divide, matching the
/// expression it replaced.
#[inline]
pub fn q32_to_unit_f64(phase: u32) -> f64 {
    phase as f64 / Q32_PER_CYCLE_F64
}

/// Per-sample Q32 phase increment for `hz` at `sample_rate`.
#[inline]
pub fn phase_inc_q32(hz: f32, sample_rate: f32) -> u32 {
    ((hz / sample_rate) * Q32_PER_CYCLE) as u32
}

#[cfg(test)]
mod q32_tests {
    use super::*;

    /// The three constants must agree, and the reciprocal must be exact —
    /// 2^32 is a power of two, so 1/2^32 is representable with no rounding.
    #[test]
    fn constants_agree_and_reciprocal_is_exact() {
        assert_eq!(Q32_PER_CYCLE as f64, Q32_PER_CYCLE_F64);
        assert_eq!(INV_Q32_PER_CYCLE, 1.0 / Q32_PER_CYCLE);
        assert_eq!(INV_Q32_PER_CYCLE * Q32_PER_CYCLE, 1.0);
    }

    /// Pins the exact expressions the call sites used before 0224 moved them,
    /// so a later "tidy-up" that swaps a divide for a multiply (or narrows the
    /// f64 form) fails here rather than in a render hash.
    #[test]
    fn helpers_match_the_literal_expressions_they_replaced() {
        for p in [0u32, 1, 0x4000_0000, 0x8000_0000, 0xFFFF_FFFF] {
            assert_eq!(q32_to_unit(p), p as f32 * (1.0 / 4_294_967_296.0));
            assert_eq!(q32_to_unit_f64(p), p as f64 / 4_294_967_296.0);
        }
        for (hz, sr) in [(440.0f32, 48_000.0f32), (20.0, 44_100.0), (12_000.0, 96_000.0)] {
            assert_eq!(phase_inc_q32(hz, sr), ((hz / sr) * 4_294_967_296.0) as u32);
        }
    }

    #[test]
    fn phase_endpoints_map_as_documented() {
        assert_eq!(q32_to_unit(0), 0.0);
        assert!((q32_to_unit(0x8000_0000) - 0.5).abs() < 1e-7);
    }
}
