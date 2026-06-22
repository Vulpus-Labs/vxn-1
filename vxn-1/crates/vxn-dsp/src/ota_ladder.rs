//! R3109/IR3109-style OTA-C ladder lowpass — a Roland/Juno-flavoured filter.
//!
//! Four TPT one-pole stages like a classic transistor ladder, but the
//! nonlinearity lives **inside each integrator** (a per-stage `tanh` on the
//! integrator input) rather than on the global feedback sum. That matches the
//! softer, more distributed saturation of OTA-C filter chips (IR3109, CEM3320,
//! …) and gives a cleaner, more sinusoidal self-oscillation than a Moog-style
//! transistor ladder.
//!
//! Differences from a Moog-style transistor ladder (`tanh` on the feedback sum):
//!
//! * Per-stage `tanh`, not a single global pre-feedback `tanh`.
//! * **No** resonance-dependent input attenuation — Juno-style filters don't
//!   thin the bass under high resonance, so there is no `scale` term and no
//!   Sharp/Smooth voicing axis. There is also **no** resonance gain
//!   compensation: this is a Juno emulation, so the `1/(1+k)` passband loss
//!   under resonance is left intact as part of the authentic OTA character.
//! * Selectable response ([`FilterMode`]): 24 / 12 dB lowpass, band-pass,
//!   high-pass and notch, all formed as the classic analogue-ladder linear
//!   combination of the four stage outputs and the ladder input node (the
//!   "filter mode mixing" of the Oberheim/SSM multimode designs). The
//!   resonance feedback loop is **always** taken from the 4th stage, so the
//!   filter self-oscillates identically at `k ≈ 4` in every mode.
//!
//! Frozen-coefficient kernel, matching VXN1's per-control-block model (see
//! crate docs); the engine recomputes coefficients once per block. The poly
//! sibling [`crate::poly::PolyOtaLadder`] additionally ramps them per sample.

#[cfg(test)]
use crate::math::fast_tanh;
use std::f32::consts::{FRAC_PI_4, PI};

/// Filter response (lowpass / highpass / bandpass / notch). The actual tap-mix
/// also depends on [`FilterSlope`] (2- vs 4-pole); see [`FilterMode::mix`].
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum FilterMode {
    /// Lowpass.
    #[default]
    Lp,
    /// Highpass.
    Hp,
    /// Bandpass.
    Bp,
    /// Notch / band-reject.
    Notch,
}

/// Filter order — the 2-pole (12 dB/oct) vs 4-pole (24 dB/oct) variant of a
/// [`FilterMode`]. The resonance feedback loop is always the 4th stage, so
/// self-oscillation is identical in both.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum FilterSlope {
    /// 12 dB/oct (2-pole).
    Pole2,
    /// 24 dB/oct (4-pole).
    #[default]
    Pole4,
}

impl FilterMode {
    pub const COUNT: usize = FilterMode::Notch as usize + 1;
    pub const ALL: [FilterMode; Self::COUNT] = [
        FilterMode::Lp,
        FilterMode::Hp,
        FilterMode::Bp,
        FilterMode::Notch,
    ];

    pub fn label(self) -> &'static str {
        match self {
            FilterMode::Lp => "LP",
            FilterMode::Hp => "HP",
            FilterMode::Bp => "BP",
            FilterMode::Notch => "Notch",
        }
    }

    /// Mix the ladder nodes into this mode's output at the given `slope`. `e` is
    /// the ladder input node (post drive + resonance feedback); `y` the four
    /// stage outputs (each a one-pole LP of the previous). These are the standard
    /// ladder-multimode combinations.
    ///
    /// Notch is the 2-pole `e − 2·y0 + 2·y1` for both slopes: its transfer
    /// function `1 − 2u + 2u²` (`u = 1/(1+jω/ω_c)`) has an *exact* zero at the
    /// cutoff regardless of resonance, and a ladder can't form a steeper notch
    /// with a comparably clean null, so the slope switch is a no-op for notch.
    #[inline]
    pub fn mix(self, slope: FilterSlope, e: f32, y: [f32; 4]) -> f32 {
        use FilterSlope::{Pole2, Pole4};
        match (self, slope) {
            (FilterMode::Lp, Pole2) => y[1],
            (FilterMode::Lp, Pole4) => y[3],
            (FilterMode::Hp, Pole2) => e - 2.0 * y[0] + y[1],
            (FilterMode::Hp, Pole4) => e - 4.0 * y[0] + 6.0 * y[1] - 4.0 * y[2] + y[3],
            (FilterMode::Bp, Pole2) => 2.0 * (y[0] - y[1]),
            (FilterMode::Bp, Pole4) => 4.0 * y[1] - 8.0 * y[2] + 4.0 * y[3],
            (FilterMode::Notch, _) => e - 2.0 * y[0] + 2.0 * y[1],
        }
    }
}

/// TPT one-pole stage gain. The four-stage ladder self-oscillates at the
/// cutoff frequency *in continuous time*, but the explicit `z⁻¹` on the
/// resonance feedback path (`y4_prev` in [`OtaLadderKernel::tick`]) adds a
/// `2π·fc/fs` phase lag around the loop. The four cascaded one-poles absorb
/// that deficit by oscillating *below* their corner — observably flat by a
/// few semitones in the kHz band at base sample rate, and dependent on the
/// oversampling ratio.
///
/// To pin self-oscillation at the nominal cutoff regardless of `fs`, detune
/// the prewarped pole upward by the inverse of the per-pole phase shift:
/// each of the four poles must contribute `π·fc/(2fs)` less lag, i.e.
/// `atan(fc / fc_pole) = π/4 − π·fc/(2fs)`, giving
/// `fc_pole = fc / tan(π/4 − π·fc/(2fs))`. One extra `tan` per coeff update.
#[inline]
pub(crate) fn compute_g(cutoff_hz: f32, sample_rate: f32) -> f32 {
    let fc = cutoff_hz.clamp(5.0, sample_rate * 0.45);
    let denom = (FRAC_PI_4 - PI * fc / (2.0 * sample_rate)).tan();
    let fc_adj = (fc / denom).min(sample_rate * 0.49);
    let wd = (PI * fc_adj / sample_rate).tan();
    (wd / (1.0 + wd)).clamp(1.0e-5, 0.999)
}

/// Frozen OTA-ladder coefficients for one control block.
#[derive(Copy, Clone, Debug)]
pub struct OtaLadderCoeffs {
    /// TPT one-pole stage gain in `(0, 1)`.
    pub g: f32,
    /// Global feedback factor in `[0, 4]` (self-oscillation at 4).
    pub k: f32,
    /// Input drive applied before stage 0's `tanh`.
    pub drive: f32,
}

impl OtaLadderCoeffs {
    /// `resonance` is taken in `[0, 1]` and scaled to the `[0, 4]` feedback
    /// range internally (self-oscillation at `resonance = 1.0`), the same
    /// `[0, 1]` → `[0, 4]` resonance convention a classic ladder coeff builder
    /// would use.
    #[inline]
    pub fn new(cutoff_hz: f32, sample_rate: f32, resonance: f32, drive: f32) -> Self {
        Self {
            g: compute_g(cutoff_hz, sample_rate),
            k: 4.0 * resonance.clamp(0.0, 1.0),
            drive: drive.max(0.0),
        }
    }
}

/// Single-voice OTA-ladder kernel. Frozen coefficients (set once per block).
///
/// Test oracle for [`crate::poly::PolyOtaLadder`]: the scalar reference
/// against which mode-mix correctness is verified in differential tests.
/// Only compiled under `#[cfg(test)]`.
#[cfg(test)]
#[derive(Clone)]
pub(crate) struct OtaLadderKernel {
    g: f32,
    k: f32,
    drive: f32,
    mode: FilterMode,
    slope: FilterSlope,
    s: [f32; 4],
    y4_prev: f32,
}

#[cfg(test)]
impl OtaLadderKernel {
    pub fn new() -> Self {
        Self {
            g: 0.5,
            k: 0.0,
            drive: 1.0,
            mode: FilterMode::Lp,
            slope: FilterSlope::Pole4,
            s: [0.0; 4],
            y4_prev: 0.0,
        }
    }

    /// Replace coefficients (call once per control block).
    #[inline]
    pub fn set_coeffs(&mut self, c: OtaLadderCoeffs) {
        self.g = c.g;
        self.k = c.k;
        self.drive = c.drive;
    }

    /// Change filter response + slope. The feedback path is unchanged, so the
    /// filter keeps ringing identically — only the output tap-mix shifts.
    #[inline]
    pub fn set_response(&mut self, mode: FilterMode, slope: FilterSlope) {
        self.mode = mode;
        self.slope = slope;
    }

    pub fn mode(&self) -> FilterMode {
        self.mode
    }

    pub fn slope(&self) -> FilterSlope {
        self.slope
    }

    pub fn reset(&mut self) {
        self.s = [0.0; 4];
        self.y4_prev = 0.0;
    }

    /// Run one sample, return the selected mode's output mix.
    #[inline]
    pub fn tick(&mut self, x: f32) -> f32 {
        let g = self.g;
        let fed = self.drive * x - self.k * self.y4_prev;
        let mut input = fed;
        let mut stages = [0.0f32; 4];
        for (i, stage) in stages.iter_mut().enumerate() {
            let u = fast_tanh(input);
            let v = (u - self.s[i]) * g;
            let yn = v + self.s[i];
            self.s[i] = yn + v;
            *stage = yn;
            input = yn;
        }
        self.y4_prev = stages[3];
        self.mode.mix(self.slope, fed, stages)
    }
}

#[cfg(test)]
impl Default for OtaLadderKernel {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
