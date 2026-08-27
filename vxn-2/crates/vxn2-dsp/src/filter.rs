//! vxn-2's OTA ladder: the shared kernel, plus **vxn-2's own** resonance voicing.
//!
//! The kernel, modes, mix tables, `compute_g` and `OtaLadderCoeffs` moved to
//! `vxn-core-dsp::filter` in ticket 0227. What stayed here is the part that is
//! not shared and should not be: [`k_cap`], the cutoff-tracked feedback ceiling.
//!
//! vxn-1 and vxn-1b deliberately do not cap — their ladders self-oscillate at
//! high cutoff. vxn-2 does, because at high cutoff the discrete ladder's
//! self-oscillation threshold falls and FM's dense inharmonic HF parks a
//! screaming peak there. Unifying the two would have changed one synth's sound,
//! which is why 0227 shared the *mechanism* (`new_capped`) and left the
//! *policy* — this table — per-synth, per ADR 0002 §6.

pub use vxn_core_dsp::filter::{
    FilterMode, FilterSlope, OtaLadderCoeffs, OtaLadderKernel, compute_g,
};

/// Cutoff-tracked feedback ceiling — the cutoff-dependent resonance damping
/// (sound-design fix, 2026-06-12). The discrete ladder's self-oscillation
/// threshold *falls* as cutoff rises (the `z⁻¹` resonance-feedback lag, see
/// [`compute_g`]): the largest feedback `k` whose ring still decays is ≈3.5 at
/// 1 kHz but only ≈1.0 by 12 kHz (measured). With the flat `k = 4·resonance`
/// that means a high (often matrix-modulated) cutoff self-oscillates at low
/// resonance settings and parks a screaming peak on the dense *inharmonic* HF
/// that FM produces — the reported "doesn't sound musical".
///
/// So we cap the effective feedback to a ceiling that tracks ~15 % under that
/// measured threshold above a knee, while leaving low/mid cutoff at the full
/// `k = 4` (self-oscillation preserved). The cap is a `min`, so moderate
/// resonance — already below the ceiling — is untouched; only the top of the
/// resonance range at high cutoff is tamed (resonates but decays instead of
/// sustaining). Breakpoints are absolute cutoff Hz (not the oversampled
/// Nyquist) so the taming is musically uniform at every oversample factor;
/// linear interpolation between, flat outside.
const K_CAP_BREAKS: [(f32, f32); 5] = [
    (3_000.0, 4.0),  // ≤3 kHz: full self-oscillation
    (5_000.0, 2.0),
    (7_000.0, 1.4),
    (9_000.0, 1.1),
    (12_000.0, 0.9), // ≥12 kHz: decays even at resonance = 1
];

#[inline]
pub fn k_cap(cutoff_hz: f32) -> f32 {
    let b = &K_CAP_BREAKS;
    let last = b.len() - 1;
    if cutoff_hz <= b[0].0 {
        return b[0].1;
    }
    if cutoff_hz >= b[last].0 {
        return b[last].1;
    }
    let mut i = 0;
    while cutoff_hz > b[i + 1].0 {
        i += 1;
    }
    let (x0, y0) = b[i];
    let (x1, y1) = b[i + 1];
    y0 + (y1 - y0) * (cutoff_hz - x0) / (x1 - x0)
}

/// Frozen OTA-ladder coefficients for one control block.

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    #[test]
    fn passes_dc_and_attenuates_hf() {
        let sr = 48_000.0;
        let mut k = OtaLadderKernel::new();
        k.set_coeffs(OtaLadderCoeffs::new_capped(1000.0, sr, 0.0, 1.0, k_cap(1000.0)));
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
        let c = OtaLadderCoeffs::new_capped(cutoff, sr, 0.0, 1.0, k_cap(cutoff));
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
        k.set_coeffs(OtaLadderCoeffs::new_capped(2000.0, sr, 1.0, 1.0, k_cap(2000.0)));
        let mut peak = 0.0f32;
        for i in 0..48_000 {
            let x = if i == 0 { 1.0 } else { 0.0 };
            let y = k.tick(x);
            assert!(y.is_finite());
            peak = peak.max(y.abs());
        }
        assert!(peak < 10.0, "self-osc blew up: {peak}");
    }

    #[test]
    fn k_cap_full_low_tamed_high_monotonic() {
        assert_eq!(k_cap(500.0), 4.0, "low cutoff must allow full self-osc feedback");
        assert_eq!(k_cap(3_000.0), 4.0);
        assert_eq!(k_cap(20_000.0), 0.9, "top must clamp to the tamed ceiling");
        // Monotonic non-increasing across the audible range.
        let mut prev = 4.0;
        let mut f = 500.0;
        while f <= 20_000.0 {
            let c = k_cap(f);
            assert!(c <= prev + 1e-6, "k_cap not monotonic at {f}: {c} > {prev}");
            prev = c;
            f += 200.0;
        }
        // The cap must sit under the measured self-osc threshold above ~5 kHz
        // (so resonance = 1 decays there) but above it at 2 kHz (self-osc kept).
        assert!(k_cap(2_000.0) >= 3.0, "self-osc lost at 2 kHz");
        assert!(k_cap(8_000.0) < 1.6, "8 kHz cap above the self-osc threshold");
        assert!(k_cap(12_000.0) < 1.0, "12 kHz cap above the self-osc threshold");
    }

    /// Three disjoint scenarios for cutoff-cap and quiescence-gate behaviour:
    ///
    /// 1. **Non-resonant decay** (quiescence gate): a ladder with
    ///    low resonance settles below the −100 dBFS skip floor within ~0.5 s
    ///    so a released voice is eventually quiescent.
    ///
    /// 2. **High-cutoff resonance decays** (cutoff-cap fix): at a high cutoff
    ///    the cutoff-tracked damping caps `k` below the self-osc threshold, so
    ///    resonance = 1 decays rather than parking a screaming HF peak.
    ///
    /// 3. **Low-cutoff self-osc sustains** (quiescence gate + cutoff-cap guard):
    ///    at a low cutoff resonance = 1 sustains its limit cycle on silence —
    ///    the feature is intact and the voice is never wrongly skipped.
    #[test]
    fn filter_resonance_decay_and_sustain_properties() {
        const EPS: f32 = 1.0e-5;
        let sr = 48_000.0;

        // 1. Non-resonant (reso=0.2, cutoff=1000 Hz): excite then silence;
        //    state must fall below the floor within ~0.5 s.
        let mut k = OtaLadderKernel::new();
        k.set_coeffs(OtaLadderCoeffs::new_capped(1000.0, sr, 0.2, 1.0, k_cap(1000.0)));
        for _ in 0..500 {
            k.tick(0.3);
        }
        let mut settled = None;
        for i in 0..(sr as usize / 2) {
            k.tick(0.0);
            if k.state_abs_max() < EPS {
                settled = Some(i);
                break;
            }
        }
        assert!(
            settled.is_some(),
            "non-resonant ladder never settled below floor: {}",
            k.state_abs_max()
        );

        // 2. High cutoff (14 kHz, reso=1): cutoff-tracked damp forces decay.
        let mut hi = OtaLadderKernel::new();
        hi.set_coeffs(OtaLadderCoeffs::new_capped(14_000.0, sr, 1.0, 1.0, k_cap(14_000.0)));
        for _ in 0..500 {
            hi.tick(0.5);
        }
        let mut hi_settled = None;
        for i in 0..(sr as usize) {
            hi.tick(0.0);
            if hi.state_abs_max() < EPS {
                hi_settled = Some(i);
                break;
            }
        }
        assert!(
            hi_settled.is_some(),
            "high-cutoff resonance still self-oscillated (state {})",
            hi.state_abs_max(),
        );

        // 3. Low cutoff (1500 Hz, reso=1): self-osc sustains — never decays.
        let mut lo = OtaLadderKernel::new();
        lo.set_coeffs(OtaLadderCoeffs::new_capped(1500.0, sr, 1.0, 1.0, k_cap(1500.0)));
        for _ in 0..500 {
            lo.tick(0.5);
        }
        let mut lo_min = f32::INFINITY;
        for _ in 0..(sr as usize) {
            lo.tick(0.0);
            lo_min = lo_min.min(lo.state_abs_max());
        }
        assert!(lo_min > EPS, "low-cutoff self-osc wrongly decayed: {lo_min}");
    }

    // Integrated per-voice path: interpolate → ladder@F → decimate (the actual
    // per-voice filter chain the engine runs), not the bare kernel. These
    // exercise the whole chain so oversampling's effect is observable.

    use crate::halfband::{Interpolator, Oversampler};

    /// Run `input` (base-rate) through the per-voice oversampled chain at
    /// `factor`: upsample → ladder at the oversampled rate → decimate.
    fn osc_chain(
        mode: FilterMode,
        slope: FilterSlope,
        cutoff: f32,
        reso: f32,
        drive: f32,
        factor: usize,
        input: &[f32],
    ) -> Vec<f32> {
        let sr = 48_000.0;
        let os_rate = sr * factor as f32;
        let mut k = OtaLadderKernel::new();
        k.set_coeffs(OtaLadderCoeffs::new_capped(cutoff, os_rate, reso, drive, k_cap(cutoff)));
        k.set_response(mode, slope);

        let n = input.len();
        let osn = n * factor;
        let mut up = vec![0.0f32; osn];
        Interpolator::new().interpolate(input, &mut up, factor);
        for s in up.iter_mut() {
            *s = k.tick(*s);
        }
        let mut down = vec![0.0f32; n];
        Oversampler::new().decimate(&up, &mut down, factor);
        down
    }

    /// `|X(f)|²` of a real signal via Goertzel (DFT bin magnitude squared).
    fn goertzel_mag2(x: &[f32], f: f32, fs: f32) -> f64 {
        let w = 2.0 * std::f64::consts::PI * f as f64 / fs as f64;
        let coeff = 2.0 * w.cos();
        let (mut s1, mut s2) = (0.0f64, 0.0f64);
        for &v in x {
            let s0 = v as f64 + coeff * s1 - s2;
            s2 = s1;
            s1 = s0;
        }
        s1 * s1 + s2 * s2 - coeff * s1 * s2
    }

    /// Fraction of in-band energy that is *inharmonic* — i.e. aliasing + noise
    /// folded onto non-harmonic bins. `f0` is chosen so the analysis window
    /// holds an integer number of periods of every harmonic (leakage-free
    /// Goertzel), so anything not at `k·f0` is genuine aliasing.
    fn inharmonic_fraction(factor: usize) -> f64 {
        let sr = 48_000.0_f32;
        let win = 4096usize;
        // f0 = sr · 200 / 4096 ≈ 2343.75 Hz: exactly 200 periods per window,
        // each harmonic k·f0 lands on bin 200·k (integer ≤ 2048 for k ≤ 10).
        let f0 = sr * 200.0 / win as f32;
        let n = 3 * win; // settle the chain, analyse the tail window
        let input: Vec<f32> = (0..n)
            .map(|i| 0.8 * (2.0 * PI * f0 * i as f32 / sr).sin())
            .collect();

        // Resonant, driven low-pass with the fundamental in-band: the ladder's
        // saturator generates harmonics that, at low F, alias back in-band.
        let out = osc_chain(FilterMode::Lp, FilterSlope::Pole4, 4000.0, 0.8, 6.0, factor, &input);
        let tail = &out[n - win..];

        let total: f64 = tail.iter().map(|&v| (v as f64) * (v as f64)).sum();
        // Parseval: per-bin time-energy = (2/N)·|X|² for a positive-freq bin.
        let nyq = sr / 2.0;
        let max_k = (nyq / f0) as usize;
        let mut harmonic = 0.0f64;
        for k in 1..=max_k {
            harmonic += (2.0 / win as f64) * goertzel_mag2(tail, k as f32 * f0, sr);
        }
        ((total - harmonic) / total).max(0.0)
    }

    /// AC 4 — oversampling strictly reduces aliasing/THD of a driven, resonant
    /// sweep. The inharmonic (aliased) energy fraction must fall monotonically
    /// 1× → 2× → 4× → 8×. dB figures are printed for the record.
    ///
    /// Recorded (driven LP4, cutoff 4 kHz, reso 0.8, drive 6×, f0 ≈ 2.34 kHz):
    /// 1× = −54.6 dB, 2× = −64.7 dB, 4× = −67.1 dB, 8× = −75.1 dB inharmonic
    /// energy — a ~20 dB alias reduction from 1× to 8×.
    #[test]
    fn aliasing_decreases_monotonically_with_oversampling() {
        let mut prev = f64::INFINITY;
        let mut db = Vec::new();
        for &factor in &[1usize, 2, 4, 8] {
            let frac = inharmonic_fraction(factor);
            db.push((factor, 10.0 * frac.log10()));
            assert!(
                frac < prev,
                "{factor}×: inharmonic fraction {frac:.6} did not drop below {prev:.6}",
            );
            prev = frac;
        }
        // Visible with `cargo test -- --nocapture`.
        for (f, d) in &db {
            println!("aliasing {f}×: inharmonic energy {d:.1} dB");
        }
    }

    /// Steady-state energy of an `f`-Hz sine through the integrated chain.
    fn chain_energy(mode: FilterMode, slope: FilterSlope, cutoff: f32, f: f32, factor: usize) -> f64 {
        let sr = 48_000.0_f32;
        let n = 8192usize;
        let input: Vec<f32> = (0..n)
            .map(|i| 0.1 * (2.0 * PI * f * i as f32 / sr).sin())
            .collect();
        let out = osc_chain(mode, slope, cutoff, 0.0, 1.0, factor, &input);
        out[n / 2..].iter().map(|&v| (v as f64) * (v as f64)).sum()
    }

    /// AC 5 — the mode/slope response holds on the *integrated* path, not just
    /// the bare kernel: a 12 dB/oct low-pass passes more HF than 24 dB/oct after
    /// the resampler round-trip.
    #[test]
    fn mode_slope_response_holds_on_oversampled_path() {
        use FilterSlope::{Pole2, Pole4};
        for &factor in &[2usize, 4, 8] {
            let lp12 = chain_energy(FilterMode::Lp, Pole2, 1000.0, 6000.0, factor);
            let lp24 = chain_energy(FilterMode::Lp, Pole4, 1000.0, 6000.0, factor);
            assert!(
                lp12 > 4.0 * lp24,
                "{factor}×: LP12 ({lp12:.3e}) not brighter than LP24 ({lp24:.3e})",
            );
        }
    }

    /// AC 5 — resonance = 1 stays finite and bounded across the cutoff range at
    /// every factor on the integrated chain (impulse + silence excitation).
    #[test]
    fn self_oscillation_bounded_every_factor_on_chain() {
        for &factor in &[1usize, 2, 4, 8] {
            for &cutoff in &[500.0f32, 2000.0, 8000.0] {
                let n = 24_000usize;
                let mut input = vec![0.0f32; n];
                input[0] = 1.0;
                let out =
                    osc_chain(FilterMode::Lp, FilterSlope::Pole4, cutoff, 1.0, 1.0, factor, &input);
                let mut peak = 0.0f32;
                for &v in &out {
                    assert!(v.is_finite(), "{factor}× cutoff {cutoff}: non-finite");
                    peak = peak.max(v.abs());
                }
                assert!(peak < 10.0, "{factor}× cutoff {cutoff}: self-osc blew up ({peak})");
            }
        }
    }
}
