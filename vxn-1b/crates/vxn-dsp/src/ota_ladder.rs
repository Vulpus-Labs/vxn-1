//! Re-export of `vxn_core_dsp::filter` — vxn-1's OTA ladder vocabulary.
//!
//! The kernel, modes, mix tables, `compute_g` and `OtaLadderCoeffs` moved to
//! `vxn-core-dsp` in ticket 0227. vxn-1's scalar `OtaLadderKernel` was
//! `#[cfg(test)]`-only — the oracle its 8-wide `PolyOtaLadder` is
//! differentially tested against — and the shared kernel now serves that role.
//!
//! `OtaLadderCoeffs::new` keeps vxn-1's **flat** `k = 4·resonance`: this
//! ladder self-oscillates at high cutoff by design. vxn-2 caps feedback there
//! (`new_capped` + its own `k_cap` table); the two had diverged and 0227
//! deliberately did not unify them. `PolyOtaLadder` stays in this crate — SoA
//! lane body, ADR 0002 §3.

pub use vxn_core_dsp::filter::{
    FilterMode, FilterSlope, OtaLadderCoeffs, OtaLadderKernel, compute_g, compute_g_into,
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    #[test]
    fn passes_dc_and_attenuates_hf() {
        let sr = 48_000.0;
        let mut k = OtaLadderKernel::new();
        k.set_coeffs(OtaLadderCoeffs::new(1000.0, sr, 0.0, 1.0));
        let x = 0.05;
        let mut last = 0.0;
        for _ in 0..2000 {
            last = k.tick(x);
        }
        assert!((last / x - 1.0).abs() < 0.02, "dc gain {}", last / x);

        k.reset();
        let mut peak = 0.0f32;
        for i in 0..2000 {
            let s = if i % 2 == 0 { x } else { -x };
            peak = peak.max(k.tick(s).abs());
        }
        assert!(peak < 0.3 * x, "hf leakage {}", peak / x);
    }

    /// Steady-state energy of a `f`-Hz sine through one mode/slope at fixed coeffs.
    fn mode_energy(mode: FilterMode, slope: FilterSlope, cutoff: f32, f: f32) -> f32 {
        let sr = 48_000.0;
        let c = OtaLadderCoeffs::new(cutoff, sr, 0.0, 1.0);
        let mut k = OtaLadderKernel::new();
        k.set_coeffs(c);
        k.set_response(mode, slope);
        let mut e = 0.0f32;
        for i in 0..4000 {
            let s = 0.1 * (2.0 * PI * f * i as f32 / sr).sin();
            let y = k.tick(s);
            if i > 2000 {
                e += y * y;
            }
        }
        e
    }

    #[test]
    fn lp12_tap_is_brighter_than_lp24() {
        // 12 dB/oct lets more HF through than 24 dB/oct. Sub-Nyquist sine well
        // above cutoff (a pure-Nyquist test is degenerate — the bilinear one-pole
        // has an exact zero at Nyquist, so both taps → 0).
        use FilterSlope::{Pole2, Pole4};
        assert!(
            mode_energy(FilterMode::Lp, Pole2, 1000.0, 6000.0)
                > 4.0 * mode_energy(FilterMode::Lp, Pole4, 1000.0, 6000.0)
        );
    }

    #[test]
    fn hp_passes_hf_blocks_lf() {
        // High-pass (both slopes): a tone well above cutoff passes; one well below
        // is attenuated.
        let cutoff = 2000.0;
        for slope in [FilterSlope::Pole2, FilterSlope::Pole4] {
            assert!(
                mode_energy(FilterMode::Hp, slope, cutoff, 8000.0)
                    > 8.0 * mode_energy(FilterMode::Hp, slope, cutoff, 200.0),
                "{slope:?}"
            );
        }
    }

    #[test]
    fn bp_rejects_lf_and_hf() {
        // Band-pass (both slopes): more energy at the centre than far below/above.
        let cutoff = 2000.0;
        for slope in [FilterSlope::Pole2, FilterSlope::Pole4] {
            let mid = mode_energy(FilterMode::Bp, slope, cutoff, cutoff);
            assert!(
                mid > 4.0 * mode_energy(FilterMode::Bp, slope, cutoff, 100.0),
                "{slope:?} lf leak"
            );
            assert!(
                mid > 4.0 * mode_energy(FilterMode::Bp, slope, cutoff, 16000.0),
                "{slope:?} hf leak"
            );
        }
    }

    #[test]
    fn notch_rejects_centre() {
        // Notch: the centre band is attenuated relative to a tone well below it.
        let cutoff = 2000.0;
        assert!(
            mode_energy(FilterMode::Notch, FilterSlope::Pole4, cutoff, 200.0)
                > 4.0 * mode_energy(FilterMode::Notch, FilterSlope::Pole4, cutoff, cutoff)
        );
    }

    #[test]
    fn stable_at_high_resonance() {
        let sr = 48_000.0;
        let mut k = OtaLadderKernel::new();
        k.set_coeffs(OtaLadderCoeffs::new(2000.0, sr, 1.0, 1.0));
        let mut peak = 0.0f32;
        for i in 0..48_000 {
            let x = if i == 0 { 1.0 } else { 0.0 };
            let y = k.tick(x);
            assert!(y.is_finite());
            peak = peak.max(y.abs());
        }
        assert!(peak < 10.0, "self-osc blew up: {peak}");
    }
}
