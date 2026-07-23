//! One-pole (−6 dB/oct) high-pass filter, placed pre-VCF in the voice-stack
//! signal chain (stack sum → HPF → musical filter → FX). Thins body / removes
//! DC + low rumble below the cutoff — a static tone-shaping HPF. Runs on a
//! stack's summed stereo pair, so it needs two scalar kernels per stack, not
//! the SoA poly form.
//!
//! Topological-preserving-transform one-pole (Zavalishin): compute the one-pole
//! *lowpass* `lp` and return `x − lp`, which is the complementary high-pass.
//! The coefficient `a = g/(1+g)` with `g = tan(π·fc/sr)` is the same mapping the
//! OTA ladder uses per stage, so HPF and musical filter share a convention.
//!
//! Coefficients are frozen per control block (set once via `set_cutoff`); the
//! HP cutoff is not a modulation destination, so no per-sample ramp is needed.

use std::f32::consts::PI;

/// Map a cutoff in Hz to the TPT one-pole coefficient `a = g/(1+g)`.
#[inline]
fn coeff(cutoff_hz: f32, sample_rate: f32) -> f32 {
    let fc = cutoff_hz.clamp(5.0, sample_rate * 0.45);
    let wd = (PI * fc / sample_rate).tan();
    (wd / (1.0 + wd)).clamp(1.0e-5, 0.999)
}

/// Single-voice one-pole high-pass kernel.
#[derive(Clone)]
pub struct HpfKernel {
    a: f32,
    s: f32,
}

impl HpfKernel {
    pub fn new() -> Self {
        Self { a: 0.0, s: 0.0 }
    }

    /// Set the cutoff (call once per control block). Runs at the base sample
    /// rate — the HP stage is deliberately *not* oversampled.
    #[inline]
    pub fn set_cutoff(&mut self, cutoff_hz: f32, sample_rate: f32) {
        self.a = coeff(cutoff_hz, sample_rate);
    }

    pub fn reset(&mut self) {
        self.s = 0.0;
    }

    /// Run one sample; returns the high-passed value `x − lowpass(x)`.
    #[inline]
    pub fn tick(&mut self, x: f32) -> f32 {
        let v = (x - self.s) * self.a;
        let lp = v + self.s;
        self.s = lp + v;
        x - lp
    }
}

impl Default for HpfKernel {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Steady-state gain at a frequency, as output peak / input peak. Both
    /// peaks are taken from the same sampled sine, so undersampling of a
    /// high-frequency tone cancels out of the ratio.
    fn peak_gain(cutoff: f32, freq: f32, sr: f32) -> f32 {
        let mut k = HpfKernel::new();
        k.set_cutoff(cutoff, sr);
        let mut out_peak = 0.0f32;
        let mut in_peak = 0.0f32;
        let n = 20_000;
        for i in 0..n {
            let x = (2.0 * PI * freq * i as f32 / sr).sin();
            let y = k.tick(x);
            // Ignore the initial transient.
            if i > n / 2 {
                out_peak = out_peak.max(y.abs());
                in_peak = in_peak.max(x.abs());
            }
        }
        out_peak / in_peak
    }

    #[test]
    fn attenuates_dc_and_lows_passes_highs() {
        let sr = 48_000.0;
        // DC is fully removed.
        let mut k = HpfKernel::new();
        k.set_cutoff(500.0, sr);
        let mut last = 0.0;
        for _ in 0..5000 {
            last = k.tick(1.0);
        }
        assert!(last.abs() < 1e-3, "DC not blocked: {last}");

        // A frequency well below cutoff is attenuated; well above passes ~unity.
        let low = peak_gain(500.0, 50.0, sr);
        let high = peak_gain(500.0, 8000.0, sr);
        assert!(low < 0.2, "low not attenuated: {low}");
        assert!(high > 0.9, "high not passed: {high}");
    }

    #[test]
    fn default_low_cutoff_is_near_transparent() {
        // At the default 20 Hz cutoff, mid/high content passes essentially
        // untouched ("off").
        let sr = 48_000.0;
        let g = peak_gain(20.0, 1000.0, sr);
        assert!(
            (g - 1.0).abs() < 0.02,
            "20 Hz HP not transparent at 1 kHz: {g}"
        );
    }
}
