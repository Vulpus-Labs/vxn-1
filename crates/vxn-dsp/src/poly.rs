//! Structure-of-arrays poly kernels for the synthesis hot path.
//!
//! Each kernel holds `[f32; MAX_VOICES]` state and processes all voices per
//! sample in a branchless loop the compiler auto-vectorises (NEON is 4-wide
//! f32, so 16 voices = 4 SIMD lanes deep). Waveform / noise colour / filter
//! variant are *global* parameters, so the `match` on them is hoisted outside
//! the lane loop — the inner loop has no data-dependent branches.
//!
//! Mirrors the design of `patches-dsp`'s poly kernels. The mono kernels in the
//! sibling modules remain for non-voice uses and as the readable reference.
//!
//! Index-based lane loops are intentional: they read/write several parallel
//! `[f32; N]` arrays in lockstep and are what the autovectoriser turns into
//! NEON. Iterator/zip forms here would obscure that, so `needless_range_loop`
//! is allowed module-wide.
#![allow(clippy::needless_range_loop)]

use crate::MAX_VOICES;
use crate::ladder::LadderCoeffs;
use crate::math::fast_sine;
use crate::noise::{NoiseColor, xorshift64};
use crate::oscillator::Waveform;

const N: usize = MAX_VOICES;

/// Branchless PolyBLEP. `dt` is floored away from zero so frozen (inactive)
/// voices can't produce NaNs; the comparison masks select the active branch.
#[inline(always)]
fn pblep(t: f32, dt: f32) -> f32 {
    let d = dt.max(1.0e-12);
    let a = t / d;
    let rise = 2.0 * a - a * a - 1.0;
    let b = (t - 1.0) / d;
    let fall = b * b + 2.0 * b + 1.0;
    let m_rise = (t < d) as u32 as f32;
    let m_fall = (t > 1.0 - d) as u32 as f32;
    rise * m_rise + fall * m_fall
}

/// Branchless `tanh` approximation: clamp to ±2.5 (where the Padé form peaks,
/// ≈0.972) then evaluate. Monotone and bounded without the early-return
/// branches of `fast_tanh`, so it vectorises.
#[inline(always)]
fn tanh_c(x: f32) -> f32 {
    let x = x.clamp(-2.5, 2.5);
    let x2 = x * x;
    let x4 = x2 * x2;
    let x6 = x4 * x2;
    x * (10395.0 + 1260.0 * x2 + 21.0 * x4)
        / (10395.0 + 4725.0 * x2 + 210.0 * x4 + 4.0 * x6)
}

#[inline(always)]
fn advance(phase: f32, inc: f32) -> f32 {
    let np = phase + inc;
    np - (np >= 1.0) as u32 as f32
}

// ── PolyOscillator ────────────────────────────────────────────────────────

/// 16-voice oscillator. Phase + increment per voice; pulse width per voice
/// (PWM modulation differs per voice).
#[derive(Clone)]
pub struct PolyOscillator {
    pub phase: [f32; N],
    pub inc: [f32; N],
}

impl Default for PolyOscillator {
    fn default() -> Self {
        Self::new()
    }
}

impl PolyOscillator {
    pub fn new() -> Self {
        Self { phase: [0.0; N], inc: [0.0; N] }
    }

    #[inline]
    pub fn reset(&mut self, v: usize) {
        self.phase[v] = 0.0;
    }

    /// Produce one sample per voice into `out`, advancing all phases. `wave` is
    /// global; `pw` is per-voice pulse width.
    #[inline]
    pub fn process(&mut self, wave: Waveform, pw: &[f32; N], out: &mut [f32; N]) {
        match wave {
            Waveform::Sine => {
                for v in 0..N {
                    out[v] = fast_sine(self.phase[v]);
                    self.phase[v] = advance(self.phase[v], self.inc[v]);
                }
            }
            Waveform::Triangle => {
                for v in 0..N {
                    let p = self.phase[v];
                    out[v] = 1.0 - 4.0 * (p - 0.5).abs();
                    self.phase[v] = advance(p, self.inc[v]);
                }
            }
            Waveform::Saw => {
                for v in 0..N {
                    let p = self.phase[v];
                    out[v] = (2.0 * p - 1.0) - pblep(p, self.inc[v]);
                    self.phase[v] = advance(p, self.inc[v]);
                }
            }
            Waveform::Pulse => {
                for v in 0..N {
                    let p = self.phase[v];
                    let dt = self.inc[v];
                    let w = pw[v];
                    let naive = 1.0 - 2.0 * (p >= w) as u32 as f32; // +1 below w, -1 above
                    let pf = {
                        let x = p - w + 1.0;
                        x - x.floor()
                    };
                    out[v] = naive + pblep(p, dt) - pblep(pf, dt);
                    self.phase[v] = advance(p, dt);
                }
            }
        }
    }
}

// ── PolyNoise ─────────────────────────────────────────────────────────────

/// 16-voice noise generator with per-voice PRNG + colour-shaping state.
#[derive(Clone)]
pub struct PolyNoise {
    state: [u64; N],
    pink0: [f32; N],
    pink1: [f32; N],
    pink2: [f32; N],
    brown: [f32; N],
}

impl PolyNoise {
    pub fn new(seed: u64) -> Self {
        let state = std::array::from_fn(|v| (seed.wrapping_add(v as u64).wrapping_mul(2_654_435_761)) | 1);
        Self { state, pink0: [0.0; N], pink1: [0.0; N], pink2: [0.0; N], brown: [0.0; N] }
    }

    pub fn reset(&mut self) {
        self.pink0 = [0.0; N];
        self.pink1 = [0.0; N];
        self.pink2 = [0.0; N];
        self.brown = [0.0; N];
    }

    /// One sample per voice into `out`. `color` is global.
    #[inline]
    pub fn process(&mut self, color: NoiseColor, out: &mut [f32; N]) {
        match color {
            NoiseColor::White => {
                for v in 0..N {
                    out[v] = xorshift64(&mut self.state[v]);
                }
            }
            NoiseColor::Pink => {
                for v in 0..N {
                    let white = xorshift64(&mut self.state[v]);
                    self.pink0[v] = 0.99765 * self.pink0[v] + white * 0.0990460;
                    self.pink1[v] = 0.96300 * self.pink1[v] + white * 0.2965164;
                    self.pink2[v] = 0.57000 * self.pink2[v] + white * 1.0526913;
                    out[v] = (self.pink0[v] + self.pink1[v] + self.pink2[v] + white * 0.1848) * 0.11;
                }
            }
            NoiseColor::Brown => {
                for v in 0..N {
                    let white = xorshift64(&mut self.state[v]);
                    self.brown[v] = (self.brown[v] + white * 0.02).clamp(-1.0, 1.0);
                    out[v] = self.brown[v];
                }
            }
        }
    }
}

// ── PolyLadder ──────────────────────────────────────────────────────────────

/// 16-voice ZDF ladder lowpass. Per-voice coefficients (cutoff is modulated
/// per voice); shared topology.
#[derive(Clone)]
pub struct PolyLadder {
    g: [f32; N],
    k: [f32; N],
    drive: [f32; N],
    scale: [f32; N],
    s0: [f32; N],
    s1: [f32; N],
    s2: [f32; N],
    s3: [f32; N],
    y4: [f32; N],
}

impl Default for PolyLadder {
    fn default() -> Self {
        Self::new()
    }
}

impl PolyLadder {
    pub fn new() -> Self {
        Self {
            g: [0.5; N],
            k: [0.0; N],
            drive: [1.0; N],
            scale: [1.0; N],
            s0: [0.0; N],
            s1: [0.0; N],
            s2: [0.0; N],
            s3: [0.0; N],
            y4: [0.0; N],
        }
    }

    pub fn reset(&mut self) {
        self.s0 = [0.0; N];
        self.s1 = [0.0; N];
        self.s2 = [0.0; N];
        self.s3 = [0.0; N];
        self.y4 = [0.0; N];
    }

    #[inline]
    pub fn set_coeffs(&mut self, v: usize, c: LadderCoeffs) {
        self.g[v] = c.g;
        self.k[v] = c.k;
        self.drive[v] = c.drive;
        self.scale[v] = c.scale;
    }

    /// One sample per voice: `out[v] = ladder(x[v])`.
    #[inline]
    pub fn process(&mut self, x: &[f32; N], out: &mut [f32; N]) {
        for v in 0..N {
            let g = self.g[v];
            let u = tanh_c(self.drive[v] * x[v] * self.scale[v] - self.k[v] * self.y4[v]);

            let v0 = (u - self.s0[v]) * g;
            let y0 = v0 + self.s0[v];
            self.s0[v] = y0 + v0;

            let v1 = (y0 - self.s1[v]) * g;
            let y1 = v1 + self.s1[v];
            self.s1[v] = y1 + v1;

            let v2 = (y1 - self.s2[v]) * g;
            let y2 = v2 + self.s2[v];
            self.s2[v] = y2 + v2;

            let v3 = (y2 - self.s3[v]) * g;
            let y3 = v3 + self.s3[v];
            self.s3[v] = y3 + v3;

            self.y4[v] = y3;
            out[v] = y3;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ladder::LadderVariant;
    use crate::oscillator::Oscillator;

    #[test]
    fn poly_saw_matches_scalar_within_tolerance() {
        // Lane 0 of the poly oscillator should track a scalar saw closely
        // (same polyblep maths, branchless form).
        let inc = 220.0 / 48_000.0;
        let mut poly = PolyOscillator::new();
        poly.inc[0] = inc;
        let mut scalar = Oscillator::new();
        scalar.set_increment(inc);

        let pw = [0.5; N];
        let mut out = [0.0; N];
        let mut max_diff = 0.0f32;
        for _ in 0..4800 {
            poly.process(Waveform::Saw, &pw, &mut out);
            let s = scalar.next(Waveform::Saw);
            max_diff = max_diff.max((out[0] - s).abs());
        }
        assert!(max_diff < 1e-5, "poly vs scalar saw diff {max_diff}");
    }

    #[test]
    fn poly_osc_all_lanes_bounded() {
        let mut poly = PolyOscillator::new();
        for v in 0..N {
            poly.inc[v] = (50.0 + v as f32 * 40.0) / 48_000.0;
        }
        let pw = [0.5; N];
        let mut out = [0.0; N];
        for wave in Waveform::ALL {
            for _ in 0..4800 {
                poly.process(wave, &pw, &mut out);
                assert!(out.iter().all(|s| s.is_finite() && s.abs() <= 2.0), "{wave:?}");
            }
        }
    }

    #[test]
    fn frozen_voice_produces_no_nan() {
        // inc = 0 (inactive voice): pblep must not divide by zero.
        let mut poly = PolyOscillator::new();
        let pw = [0.5; N];
        let mut out = [0.0; N];
        for _ in 0..100 {
            poly.process(Waveform::Pulse, &pw, &mut out);
            assert!(out.iter().all(|s| s.is_finite()));
        }
    }

    #[test]
    fn poly_ladder_stable_and_lowpass() {
        let sr = 48_000.0;
        let mut lad = PolyLadder::new();
        for v in 0..N {
            lad.set_coeffs(v, LadderCoeffs::new(1000.0, sr, 0.5, 1.0, LadderVariant::Sharp));
        }
        // Feed Nyquist-ish into all lanes; should be attenuated and finite.
        let mut peak = 0.0f32;
        let mut out = [0.0; N];
        for i in 0..4800 {
            let s = if i % 2 == 0 { 0.1 } else { -0.1 };
            let x = [s; N];
            lad.process(&x, &mut out);
            peak = peak.max(out[0].abs());
            assert!(out.iter().all(|y| y.is_finite()));
        }
        assert!(peak < 0.1, "hf not attenuated: {peak}");
    }

    #[test]
    fn poly_noise_colors_bounded() {
        let mut n = PolyNoise::new(7);
        let mut out = [0.0; N];
        for color in NoiseColor::ALL {
            for _ in 0..10_000 {
                n.process(color, &mut out);
                assert!(out.iter().all(|s| s.is_finite() && s.abs() <= 1.5), "{color:?}");
            }
        }
    }
}
