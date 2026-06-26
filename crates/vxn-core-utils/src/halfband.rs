//! Halfband FIR decimator and a 2×/4×/8× oversampling helper.
//!
//! Shared by both synths (E027/0118). A `HalfbandFir` is a 33-tap symmetric
//! linear-phase halfband filter (8 non-zero off-centre taps + centre, every
//! other tap zero by the halfband property). `process(a, b)` consumes two
//! oversampled samples and returns one band-limited, decimated sample (>60 dB
//! stopband, ~0.1 dB passband ripple, group delay 16 oversampled samples).
//!
//! Only the *decimation* half lives here — both synths need it. VXN2's
//! interpolating counterpart (`HalfbandInterp` / `Interpolator`, ticket 0082)
//! is built on this same tap table but stays in `vxn2-dsp::halfband`.

/// Non-zero off-centre taps for the default 33-tap halfband FIR.
pub const DEFAULT_TAPS: [f32; 8] = [
    -0.00188788,
    0.00386248,
    -0.00824247,
    0.01594711,
    -0.02867656,
    0.05071856,
    -0.09801591,
    0.31594176,
];
pub const DEFAULT_CENTRE: f32 = 0.500_705_8;

/// Symmetric linear-phase halfband FIR used as a 2× decimator.
#[derive(Clone)]
pub struct HalfbandFir {
    taps: Vec<f32>,
    delay: Vec<f32>,
    pos: usize,
    mask: usize,
    centre: f32,
    midpoint_offset: usize,
}

impl HalfbandFir {
    /// Group delay in oversampled samples (half the filter order). Read by the
    /// latency-reporting path (ticket 0086).
    pub const GROUP_DELAY_OVERSAMPLED: usize = 16;

    pub fn new(taps: Vec<f32>, centre: f32) -> Self {
        let taps_len = taps.len();
        let len = (taps_len * 4 + 2).next_power_of_two();
        Self {
            taps,
            delay: vec![0.0; len],
            pos: 0,
            mask: len - 1,
            centre,
            midpoint_offset: len - (taps_len * 2),
        }
    }

    pub fn reset(&mut self) {
        self.delay.iter_mut().for_each(|s| *s = 0.0);
        self.pos = 0;
    }

    /// Copy the tap-delay buffer and write position from `src` into `self`,
    /// leaving the (immutable) tap coefficients alone. Used to warm-start a
    /// parallel filter instance from a converged sibling — see
    /// [`Oversampler::clone_state_from`] for the engine-level motivation.
    pub fn clone_state_from(&mut self, src: &Self) {
        debug_assert_eq!(self.delay.len(), src.delay.len());
        self.delay.copy_from_slice(&src.delay);
        self.pos = src.pos;
    }

    /// Decimate two oversampled input samples into one output sample.
    #[inline]
    pub fn process(&mut self, first: f32, second: f32) -> f32 {
        let n_taps = self.taps.len();
        let mask = self.mask;

        let newest = self.push_sample(first);
        self.push_sample(second);

        let center_idx = (newest + self.midpoint_offset) & mask;
        let mut acc = self.centre * self.delay[center_idx];

        let mut offset_r = (center_idx + 1) & mask;
        let mut offset_l = (center_idx + mask) & mask;

        for t in (0..n_taps).rev() {
            acc += self.taps[t] * (self.delay[offset_l] + self.delay[offset_r]);
            offset_r = (offset_r + 2) & mask;
            offset_l = (offset_l + mask - 1) & mask;
        }
        acc
    }

    #[inline]
    fn push_sample(&mut self, x: f32) -> usize {
        let idx = self.pos;
        self.delay[idx] = x;
        self.pos = (self.pos + 1) & self.mask;
        idx
    }
}

impl Default for HalfbandFir {
    fn default() -> Self {
        Self::new(DEFAULT_TAPS.to_vec(), DEFAULT_CENTRE)
    }
}

/// Base-rate-referred latency, in samples, of the interpolate → filter →
/// decimate round-trip at oversample `factor` ∈ {1, 2, 4, 8} (ticket 0086).
///
/// Both the up and the down cascade are symmetric: each is `log2(factor)`
/// halfband stages, stage *i* running at `2ⁱ×` base rate and contributing
/// [`HalfbandFir::GROUP_DELAY_OVERSAMPLED`] samples *at its own rate*. Summing
/// `GROUP_DELAY / 2ⁱ` over the cascade is the geometric series
/// `GROUP_DELAY · (1 − 1/factor)` base-rate samples per direction, so the
/// round-trip is twice that:
///
/// ```text
/// latency(factor) = 2 · GROUP_DELAY_OVERSAMPLED · (factor − 1) / factor
/// ```
///
/// which is exact integer division for every power-of-two factor
/// (32·1/2 = 16, 32·3/4 = 24, 32·7/8 = 28). 1× is a passthrough copy with no
/// filtering and reports 0. The figure is *derived* from the cascade's
/// group-delay constant, never hardcoded twice (ticket 0086 acceptance).
pub const fn roundtrip_latency_base_samples(factor: usize) -> u32 {
    match factor {
        2 | 4 | 8 => {
            let g = HalfbandFir::GROUP_DELAY_OVERSAMPLED as u32;
            (2 * g * (factor as u32 - 1)) / factor as u32
        }
        _ => 0,
    }
}

/// 2× / 4× / 8× oversampling decimator. Holds three cascaded halfband stages,
/// each a 2:1 step run at successively lower rates: 8× runs stage A (8→4),
/// stage B (4→2), stage C (2→1); 4× uses A (4→2) then B (2→1); 2× uses A only.
/// A given stage always operates at the same rate regardless of factor, so its
/// filter state stays coherent.
#[derive(Clone)]
pub struct Oversampler {
    stage_a: HalfbandFir,
    stage_b: HalfbandFir,
    stage_c: HalfbandFir,
}

impl Default for Oversampler {
    fn default() -> Self {
        Self::new()
    }
}

impl Oversampler {
    pub fn new() -> Self {
        Self {
            stage_a: HalfbandFir::default(),
            stage_b: HalfbandFir::default(),
            stage_c: HalfbandFir::default(),
        }
    }

    pub fn reset(&mut self) {
        self.stage_a.reset();
        self.stage_b.reset();
        self.stage_c.reset();
    }

    /// Copy the FIR state of every stage from `src`. Used to seed a
    /// dormant R-channel decimator from its converged L-channel sibling
    /// on the `spread = 0` → `spread > 0` transition, so R starts
    /// bit-identical to L instead of from cold state.
    pub fn clone_state_from(&mut self, src: &Self) {
        self.stage_a.clone_state_from(&src.stage_a);
        self.stage_b.clone_state_from(&src.stage_b);
        self.stage_c.clone_state_from(&src.stage_c);
    }

    /// Decimate `input` (length `output.len() * factor`) into `output`.
    /// `factor` must be 1, 2, 4 or 8. For 1× this is a straight copy.
    pub fn decimate(&mut self, input: &[f32], output: &mut [f32], factor: usize) {
        match factor {
            2 => {
                for (i, out) in output.iter_mut().enumerate() {
                    *out = self.stage_a.process(input[2 * i], input[2 * i + 1]);
                }
            }
            4 => {
                for (i, out) in output.iter_mut().enumerate() {
                    let base = 4 * i;
                    let a = self.stage_a.process(input[base], input[base + 1]);
                    let b = self.stage_a.process(input[base + 2], input[base + 3]);
                    *out = self.stage_b.process(a, b);
                }
            }
            8 => {
                for (i, out) in output.iter_mut().enumerate() {
                    let base = 8 * i;
                    // 8 → 4 (stage A)
                    let a0 = self.stage_a.process(input[base], input[base + 1]);
                    let a1 = self.stage_a.process(input[base + 2], input[base + 3]);
                    let a2 = self.stage_a.process(input[base + 4], input[base + 5]);
                    let a3 = self.stage_a.process(input[base + 6], input[base + 7]);
                    // 4 → 2 (stage B)
                    let b0 = self.stage_b.process(a0, a1);
                    let b1 = self.stage_b.process(a2, a3);
                    // 2 → 1 (stage C)
                    *out = self.stage_c.process(b0, b1);
                }
            }
            _ => {
                output.copy_from_slice(&input[..output.len()]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dc_passes_through_2x() {
        let mut os = Oversampler::new();
        let input = [1.0f32; 64];
        let mut output = [0.0f32; 32];
        // Flush, then measure.
        for _ in 0..4 {
            os.decimate(&input, &mut output, 2);
        }
        let tail = output[output.len() - 4..].iter().sum::<f32>() / 4.0;
        assert!((tail - 1.0).abs() < 0.01, "2x DC gain {tail}");
    }

    #[test]
    fn nyquist_rejected_2x() {
        // Alternating ±1 at the oversampled rate is the oversampled Nyquist;
        // a halfband decimator should crush it.
        let mut os = Oversampler::new();
        let input: [f32; 64] = std::array::from_fn(|i| if i % 2 == 0 { 1.0 } else { -1.0 });
        let mut output = [0.0f32; 32];
        for _ in 0..6 {
            os.decimate(&input, &mut output, 2);
        }
        let peak = output[output.len() - 8..]
            .iter()
            .fold(0.0f32, |m, &x| m.max(x.abs()));
        assert!(peak < 0.05, "2x Nyquist leakage {peak}");
    }

    #[test]
    fn dc_passes_through_4x() {
        let mut os = Oversampler::new();
        let input = [1.0f32; 128];
        let mut output = [0.0f32; 32];
        for _ in 0..6 {
            os.decimate(&input, &mut output, 4);
        }
        let tail = output[output.len() - 4..].iter().sum::<f32>() / 4.0;
        assert!((tail - 1.0).abs() < 0.02, "4x DC gain {tail}");
    }

    #[test]
    fn dc_passes_through_8x() {
        let mut os = Oversampler::new();
        let input = [1.0f32; 256];
        let mut output = [0.0f32; 32];
        for _ in 0..6 {
            os.decimate(&input, &mut output, 8);
        }
        let tail = output[output.len() - 4..].iter().sum::<f32>() / 4.0;
        assert!((tail - 1.0).abs() < 0.02, "8x DC gain {tail}");
    }

    #[test]
    fn nyquist_rejected_8x() {
        // Alternating ±1 at the 8× rate is the oversampled Nyquist; the cascade
        // should crush it back to the base rate.
        let mut os = Oversampler::new();
        let input: [f32; 256] = std::array::from_fn(|i| if i % 2 == 0 { 1.0 } else { -1.0 });
        let mut output = [0.0f32; 32];
        for _ in 0..6 {
            os.decimate(&input, &mut output, 8);
        }
        let peak = output[output.len() - 8..]
            .iter()
            .fold(0.0f32, |m, &x| m.max(x.abs()));
        assert!(peak < 0.05, "8x Nyquist leakage {peak}");
    }

    #[test]
    fn passthrough_1x_is_identity() {
        let mut os = Oversampler::new();
        let input: [f32; 32] = std::array::from_fn(|i| i as f32);
        let mut output = [0.0f32; 32];
        os.decimate(&input, &mut output, 1);
        assert_eq!(input, output);
    }

    /// Deferred decimation (ADR 0004 §4): the decimator is linear, so
    /// decimating a *sum* of voices once equals summing per-voice decimations
    /// — exactly (within f32 rounding), not approximately. This is what
    /// licenses running one shared decimator on the oversampled voice-sum bus
    /// instead of N per-voice decimators.
    #[test]
    fn decimate_is_linear_over_voice_sum() {
        for factor in [2usize, 4, 8] {
            let n = 256;
            let osn = n * factor;
            // Two distinct "voices" at the oversampled rate.
            let a: Vec<f32> = (0..osn)
                .map(|i| (i as f32 * 0.013).sin() * 0.6 + (i as f32 * 0.21).sin() * 0.2)
                .collect();
            let b: Vec<f32> = (0..osn)
                .map(|i| (i as f32 * 0.047).sin() * 0.5 - (i as f32 * 0.005).sin() * 0.3)
                .collect();
            let sum: Vec<f32> = a.iter().zip(&b).map(|(x, y)| x + y).collect();

            let mut da = vec![0.0f32; n];
            let mut db = vec![0.0f32; n];
            let mut dsum = vec![0.0f32; n];
            // Separate decimators, each from zero state (mirrors per-voice vs
            // deferred-shared; LTI ⇒ same result).
            Oversampler::new().decimate(&a, &mut da, factor);
            Oversampler::new().decimate(&b, &mut db, factor);
            Oversampler::new().decimate(&sum, &mut dsum, factor);

            let mut max_diff = 0.0f32;
            for i in 0..n {
                max_diff = max_diff.max((dsum[i] - (da[i] + db[i])).abs());
            }
            assert!(max_diff < 1e-5, "{factor}× decimate non-linear: max diff {max_diff}");
        }
    }

    #[test]
    fn roundtrip_latency_values() {
        assert_eq!(roundtrip_latency_base_samples(1), 0);
        assert_eq!(roundtrip_latency_base_samples(2), 16);
        assert_eq!(roundtrip_latency_base_samples(4), 24);
        assert_eq!(roundtrip_latency_base_samples(8), 28);
        // Any factor outside {1,2,4,8} is treated as the 1× passthrough.
        assert_eq!(roundtrip_latency_base_samples(3), 0);
        // Derived from the group-delay constant, not a hardcoded number.
        let g = HalfbandFir::GROUP_DELAY_OVERSAMPLED as u32;
        for f in [2u32, 4, 8] {
            assert_eq!(roundtrip_latency_base_samples(f as usize), 2 * g * (f - 1) / f);
        }
    }
}
