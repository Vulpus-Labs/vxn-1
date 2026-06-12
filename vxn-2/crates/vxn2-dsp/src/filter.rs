//! R3109/IR3109-style OTA-C ladder lowpass — a Roland/Juno-flavoured filter.
//!
//! Lifted from VXN1 (`vxn-dsp/src/ota_ladder.rs`) into `vxn2-dsp` as a
//! self-contained, dependency-free module (see `smoother.rs`, "lifted from
//! VXN1"). VXN2 is pure-FM with no subtractive filter and no `tanh`; this brings
//! the OTA-C ladder in as the optional per-voice filter of E007 / ADR 0004.
//!
//! Four TPT one-pole stages, but the nonlinearity lives **inside each
//! integrator** (a per-stage `tanh` on the integrator input) rather than on the
//! global feedback sum. That matches the softer, more distributed saturation of
//! OTA-C filter chips (IR3109, CEM3320, …) and gives a cleaner, more sinusoidal
//! self-oscillation than a Moog-style transistor ladder:
//!
//! * Per-stage `tanh`, not a single global pre-feedback `tanh`.
//! * **No** resonance-dependent input attenuation — Juno-style filters don't
//!   thin the bass under high resonance, so there is no `scale` term and no
//!   Sharp/Smooth voicing axis.
//! * Selectable response ([`FilterMode`]): 24 / 12 dB lowpass, band-pass,
//!   high-pass and notch, all formed as the classic analogue-ladder linear
//!   combination of the four stage outputs and the ladder input node. The
//!   resonance feedback loop is **always** taken from the 4th stage, so the
//!   filter self-oscillates identically at `k ≈ 4` in every mode.
//!
//! Frozen-coefficient kernel, matching VXN2's per-control-block model: the
//! engine recomputes coefficients once per block via [`OtaLadderCoeffs`]. The
//! per-sample-ramped poly SoA sibling from VXN1 is deliberately **not** ported —
//! the filter runs on a stack's summed stereo pair (two scalar kernels, L/R),
//! so there is no per-lane SoA problem here.

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
///
/// `sample_rate` here is the **oversampled** rate on the filter path, so the
/// `fs`-dependent pole detune stays correct at every oversample factor.
#[inline]
pub(crate) fn compute_g(cutoff_hz: f32, sample_rate: f32) -> f32 {
    let fc = cutoff_hz.clamp(5.0, sample_rate * 0.45);
    let denom = (FRAC_PI_4 - PI * fc / (2.0 * sample_rate)).tan();
    let fc_adj = (fc / denom).min(sample_rate * 0.49);
    let wd = (PI * fc_adj / sample_rate).tan();
    (wd / (1.0 + wd)).clamp(1.0e-5, 0.999)
}

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
pub(crate) fn k_cap(cutoff_hz: f32) -> f32 {
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
    /// (ticket 0083) feeds `[0, 1]` directly. `sample_rate` is the oversampled
    /// rate on the filter path.
    #[inline]
    pub fn new(cutoff_hz: f32, sample_rate: f32, resonance: f32, drive: f32) -> Self {
        Self {
            g: compute_g(cutoff_hz, sample_rate),
            // Cutoff-tracked ceiling keeps the feedback below the (falling)
            // self-osc threshold as cutoff climbs into the inharmonic-HF danger
            // zone, while leaving low/mid cutoff and moderate resonance
            // untouched (see `k_cap`).
            k: (4.0 * resonance.clamp(0.0, 1.0)).min(k_cap(cutoff_hz)),
            drive: drive.max(0.0),
        }
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
    /// integrators plus the feedback-tap memory). Ticket 0085 keys its
    /// quiescence-skip on this: a stack whose input is zero *and* whose filter
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

    /// AC for the high-resonance fix: at a *low* cutoff, resonance = 1 still
    /// self-oscillates (the limit cycle sustains on silence) — the feature is
    /// intact. At a *high* cutoff the cutoff-tracked damping caps `k` below the
    /// self-osc threshold, so the same resonance decays away instead of parking
    /// a screaming peak on inharmonic HF. This is what stops FM's dense HF
    /// content from being self-oscillated unmusically.
    #[test]
    fn high_cutoff_resonance_decays_while_low_cutoff_sustains() {
        const EPS: f32 = 1.0e-5;
        let sr = 48_000.0;

        // Low cutoff (below the damp onset): self-oscillation sustains.
        let mut lo = OtaLadderKernel::new();
        lo.set_coeffs(OtaLadderCoeffs::new(1500.0, sr, 1.0, 1.0));
        for _ in 0..500 {
            lo.tick(0.5);
        }
        let mut lo_min = f32::INFINITY;
        for _ in 0..(sr as usize) {
            lo.tick(0.0);
            lo_min = lo_min.min(lo.state_abs_max());
        }
        assert!(lo_min > EPS, "low-cutoff self-osc wrongly decayed: {lo_min}");

        // High cutoff (past the damp floor): same resonance now decays.
        let mut hi = OtaLadderKernel::new();
        hi.set_coeffs(OtaLadderCoeffs::new(14_000.0, sr, 1.0, 1.0));
        for _ in 0..500 {
            hi.tick(0.5);
        }
        let mut settled = None;
        for i in 0..(sr as usize) {
            hi.tick(0.0);
            if hi.state_abs_max() < EPS {
                settled = Some(i);
                break;
            }
        }
        assert!(
            settled.is_some(),
            "high-cutoff resonance still self-oscillated (state {})",
            hi.state_abs_max(),
        );
    }

    /// Quiescence gate (ticket 0085). A non-resonant ladder, once its input goes
    /// silent, settles below the engine's −100 dBFS skip floor in finite time —
    /// so a released voice eventually reads quiescent and can be skipped. A
    /// self-oscillating ladder (resonance → 1) sustains its limit cycle on zero
    /// input forever, so `state_abs_max` never falls below the floor and the
    /// voice is never wrongly skipped while it rings.
    #[test]
    fn state_decays_below_floor_then_self_osc_never_does() {
        const EPS: f32 = 1.0e-5;
        let sr = 48_000.0;

        // Low resonance: excite with a step, then feed silence. State must fall
        // below the floor within ~0.5 s (well inside a release tail).
        let mut k = OtaLadderKernel::new();
        k.set_coeffs(OtaLadderCoeffs::new(1000.0, sr, 0.2, 1.0));
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

        // Self-oscillation: same silence, but the limit cycle sustains — state
        // stays above the floor for a full second, so it is never skipped.
        let mut osc = OtaLadderKernel::new();
        osc.set_coeffs(OtaLadderCoeffs::new(2000.0, sr, 1.0, 1.0));
        for _ in 0..500 {
            osc.tick(0.5);
        }
        let mut min_state = f32::INFINITY;
        for _ in 0..(sr as usize) {
            osc.tick(0.0);
            min_state = min_state.min(osc.state_abs_max());
        }
        assert!(
            min_state > EPS,
            "self-oscillating ladder dipped below skip floor: {min_state}"
        );
    }

    // ---- Integrated per-voice path (E007 ticket 0087) -------------------
    //
    // The "integrated path" is interpolate → ladder@F → decimate (the actual
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
        k.set_coeffs(OtaLadderCoeffs::new(cutoff, os_rate, reso, drive));
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
        // Recorded for the ticket / README; visible with `cargo test -- --nocapture`.
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
