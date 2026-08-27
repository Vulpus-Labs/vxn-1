//! OTA-C ladder lowpass, the optional per-voice filter of ADR 0004.
//!
//! Four TPT one-pole stages, but the nonlinearity lives **inside each
//! integrator** (a per-stage `tanh` on the integrator input) rather than on the
//! global feedback sum. That gives a softer, more distributed saturation and a
//! cleaner, more sinusoidal self-oscillation than a global-feedback ladder:
//!
//! * Per-stage `tanh`, not a single global pre-feedback `tanh`.
//! * **No** resonance-dependent input attenuation, so there is no `scale` term
//!   and no Sharp/Smooth voicing axis. There is also **no** resonance gain
//!   compensation: the `1/(1+k)` passband loss under resonance is left intact.
//!   ([`k_cap`] still tames high-cutoff self-oscillation — a stability fix, not
//!   a level restore.)
//! * Selectable response ([`FilterMode`]): 24 / 12 dB lowpass, band-pass,
//!   high-pass and notch, all formed as the classic analogue-ladder linear
//!   combination of the four stage outputs and the ladder input node. The
//!   resonance feedback loop is **always** taken from the 4th stage, so the
//!   filter self-oscillates identically at `k ≈ 4` in every mode.
//!
//! Frozen-coefficient kernel on the per-control-block model: the engine
//! recomputes coefficients once per block via [`OtaLadderCoeffs`]. The filter
//! runs on a stack's summed stereo pair (two scalar kernels, L/R), so there is
//! no per-lane SoA problem here.//!
//! **Provenance** (0227). vxn-1 and vxn-2 both had this kernel; the bodies were
//! identical apart from vxn-2's `state_abs_max` (its quiescence-skip tap), and
//! vxn-1's was `#[cfg(test)]`-only, serving as the scalar oracle for its 8-wide
//! `PolyOtaLadder`. What had NOT stayed identical was the coefficient builder:
//! vxn-2 caps feedback at high cutoff, vxn-1 does not. That divergence is a
//! voicing decision, so it stays per-synth — see [`OtaLadderCoeffs::new_capped`].
//! `PolyOtaLadder` itself does not move; ADR 0002 §3 keeps SoA lane bodies
//! per-synth.

use vxn_core_utils::math::fast_tanh;
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
///
/// `sample_rate` here is the **oversampled** rate on the filter path, so the
/// `fs`-dependent pole detune stays correct at every oversample factor.
#[inline]
pub fn compute_g(cutoff_hz: f32, sample_rate: f32) -> f32 {
    let fc = cutoff_hz.clamp(5.0, sample_rate * 0.45);
    let denom = (FRAC_PI_4 - PI * fc / (2.0 * sample_rate)).tan();
    let fc_adj = (fc / denom).min(sample_rate * 0.49);
    let wd = (PI * fc_adj / sample_rate).tan();
    (wd / (1.0 + wd)).clamp(1.0e-5, 0.999)
}

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
    /// range internally (self-oscillation at `resonance = 1.0`). The param layer
    /// feeds `[0, 1]` directly. `sample_rate` is the oversampled rate on the
    /// filter path.
    #[inline]
    pub fn new(cutoff_hz: f32, sample_rate: f32, resonance: f32, drive: f32) -> Self {
        Self {
            g: compute_g(cutoff_hz, sample_rate),
            k: 4.0 * resonance.clamp(0.0, 1.0),
            drive: drive.max(0.0),
        }
    }

    /// As [`new`](Self::new), with the feedback additionally capped at `max_k`.
    ///
    /// The cap is a **mechanism**; deciding whether to apply one, and what the
    /// ceiling should be at each cutoff, is a voicing decision and stays with
    /// the synth (ADR 0002 §6). vxn-2 passes `k_cap(cutoff_hz)` from its own
    /// breakpoint table; vxn-1 and vxn-1b deliberately do not cap, and
    /// self-oscillate at high cutoff.
    ///
    /// This split is why 0227 could not simply unify the constructor: the two
    /// synths' ladders had genuinely diverged, and sharing one `new` would have
    /// changed one of them audibly.
    #[inline]
    pub fn new_capped(
        cutoff_hz: f32,
        sample_rate: f32,
        resonance: f32,
        drive: f32,
        max_k: f32,
    ) -> Self {
        let mut c = Self::new(cutoff_hz, sample_rate, resonance, drive);
        c.k = c.k.min(max_k);
        c
    }
}

/// Single-voice OTA-ladder kernel. Frozen coefficients (set once per block).
#[derive(Clone)]
pub struct OtaLadderKernel {
    g: f32,
    k: f32,
    drive: f32,
    mode: FilterMode,
    slope: FilterSlope,
    s: [f32; 4],
    y4_prev: f32,
}

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

    /// Largest absolute value across all internal state (the four ladder stage
    /// integrators plus the feedback-tap memory). The quiescence-skip keys on
    /// this: a stack whose input is zero *and* whose filter
    /// state has fallen below an audibility floor can be skipped, because its
    /// future output is bounded by this magnitude. A self-oscillating filter
    /// (resonance → 1) sustains large state forever, so it never reads as
    /// quiescent and is never wrongly skipped.
    #[inline]
    pub fn state_abs_max(&self) -> f32 {
        let mut m = self.y4_prev.abs();
        for &v in &self.s {
            m = m.max(v.abs());
        }
        m
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

impl Default for OtaLadderKernel {
    fn default() -> Self {
        Self::new()
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    /// The distinction 0227 turns on: `new` is flat, `new_capped` applies a
    /// ceiling, and they agree wherever the ceiling is not binding.
    #[test]
    fn new_is_flat_and_new_capped_only_binds_above_the_ceiling() {
        // Ceiling well above 4·resonance — no effect.
        let flat = OtaLadderCoeffs::new(1_000.0, SR, 1.0, 1.0);
        let capped = OtaLadderCoeffs::new_capped(1_000.0, SR, 1.0, 1.0, 4.0);
        assert_eq!(flat.k, 4.0);
        assert_eq!(capped.k, flat.k, "a non-binding cap must change nothing");
        assert_eq!(capped.g, flat.g);
        assert_eq!(capped.drive, flat.drive);

        // Ceiling below it — binds.
        let tamed = OtaLadderCoeffs::new_capped(12_000.0, SR, 1.0, 1.0, 0.9);
        assert_eq!(tamed.k, 0.9);
        assert_eq!(
            OtaLadderCoeffs::new(12_000.0, SR, 1.0, 1.0).k,
            4.0,
            "vxn-1's uncapped ladder must still self-oscillate at high cutoff"
        );
    }

    #[test]
    fn resonance_and_drive_are_clamped() {
        assert_eq!(OtaLadderCoeffs::new(1_000.0, SR, 5.0, 1.0).k, 4.0);
        assert_eq!(OtaLadderCoeffs::new(1_000.0, SR, -1.0, 1.0).k, 0.0);
        assert_eq!(OtaLadderCoeffs::new(1_000.0, SR, 0.5, -3.0).drive, 0.0);
    }

    /// Small-signal only: each stage has a `tanh` on its integrator input, so a
    /// unity-amplitude input saturates and DC gain falls well below 1. That is
    /// the design, not a defect — measure the linear region.
    #[test]
    fn passes_dc_and_attenuates_hf() {
        let x = 0.05;
        let mut k = OtaLadderKernel::new();
        k.set_coeffs(OtaLadderCoeffs::new(1_000.0, SR, 0.0, 1.0));
        let mut last = 0.0;
        for _ in 0..2_000 {
            last = k.tick(x);
        }
        assert!((last / x - 1.0).abs() < 0.02, "dc gain {}", last / x);

        k.reset();
        let mut peak = 0.0f32;
        for i in 0..2_000 {
            let s = if i % 2 == 0 { x } else { -x };
            peak = peak.max(k.tick(s).abs());
        }
        assert!(peak < 0.3 * x, "hf leakage {}", peak / x);
    }

    #[test]
    fn stays_finite_and_bounded_at_full_resonance() {
        let mut k = OtaLadderKernel::new();
        k.set_coeffs(OtaLadderCoeffs::new(2_000.0, SR, 1.0, 1.0));
        let mut peak = 0.0f32;
        for i in 0..48_000 {
            let y = k.tick(if i == 0 { 1.0 } else { 0.0 });
            assert!(y.is_finite());
            peak = peak.max(y.abs());
        }
        assert!(peak < 10.0, "self-osc blew up: {peak}");
    }

    /// vxn-2's quiescence skip keys on this; a self-oscillating filter must
    /// never read as quiescent or it would be wrongly skipped.
    #[test]
    fn state_abs_max_tracks_a_ringing_filter() {
        let mut k = OtaLadderKernel::new();
        k.set_coeffs(OtaLadderCoeffs::new(2_000.0, SR, 1.0, 1.0));
        assert_eq!(k.state_abs_max(), 0.0, "a fresh kernel holds no state");
        k.tick(1.0);
        assert!(k.state_abs_max() > 0.0, "state should be non-zero after an impulse");
    }
}
