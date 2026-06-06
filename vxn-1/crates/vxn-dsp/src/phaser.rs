//! Stereo allpass phaser, ported from `patches-bundles::patches-vintage`
//! (`VStereoPhaser` + `VPhaserCore`).
//!
//! Two [`PhaserChannel`] cascades share one triangle LFO; the right channel
//! reads the LFO at a fixed anti-phase offset (the upstream's `spread = 1.0`
//! mode — the headline swirling stereo motion). The upstream macro surface
//! (rate, depth, center, feedback, mix, spread, width, jitter, stages) is
//! collapsed to **Rate / Depth / FB / Mix**; everything else is pinned:
//!
//! - `STAGES = 4` (4 allpass per channel → 2 notches)
//! - `SPREAD = 1.0` (anti-phase L/R sweep)
//! - `WIDTH = 1.0` (no mid/side scaling — drops the M/S transform entirely)
//! - `JITTER = 0.0` (deterministic; analog drift handled at master level)
//! - `CENTER_HZ = 600.0`
//!
//! Coefficient cost is hoisted off the per-sample path: the swept break
//! frequency is sampled and `tan`-mapped every [`CONTROL_INTERVAL`] samples
//! and linearly ramped between updates. The LFO ticks per sample so the
//! sweep stays smooth.

use crate::flush_denormal;
use crate::math::{fast_tanh, xorshift64};

// ── Pinned structural constants ─────────────────────────────────────────────

/// Allpass stages per channel. Notches = STAGES / 2.
const STAGES: usize = 4;
/// Feedback magnitude clamp — keeps the resonant peak below self-oscillation.
const FB_MAX: f32 = 0.9;
/// Samples between allpass-coefficient recomputes. 16 @ 48 kHz ≈ 0.33 ms.
const CONTROL_INTERVAL: u32 = 16;
/// Per-stage break-frequency tolerance: ±3 % spread, modelling component
/// scatter so notches aren't perfectly harmonic.
const STAGE_SPREAD: f32 = 0.03;
/// Centre of the swept band, Hz. Mid-band — sits between vocal and presence.
const CENTER_HZ: f32 = 600.0;

// ── Triangle LFO ────────────────────────────────────────────────────────────

#[derive(Clone)]
struct TriangleLfo {
    phase: f32,
    increment: f32,
}

impl TriangleLfo {
    fn new() -> Self {
        Self {
            phase: 0.0,
            increment: 0.0,
        }
    }

    fn set_rate(&mut self, rate_hz: f32, sample_rate: f32) {
        self.increment = rate_hz / sample_rate;
    }

    /// Triangle value in `[-1, +1]` at an arbitrary (unwrapped) phase.
    #[inline]
    fn triangle(phase: f32) -> f32 {
        let p = phase - phase.floor();
        (4.0 * (p - (p + 0.5).floor()).abs() - 1.0).clamp(-1.0, 1.0)
    }

    /// Advance one sample and return the triangle at the current phase and at
    /// `phase + offset_cycles` (a fraction of a cycle, wrapped). The R-channel
    /// reads at `+0.5` for the pinned anti-phase sweep.
    #[inline]
    fn tick_offset(&mut self, offset_cycles: f32) -> (f32, f32) {
        self.phase += self.increment;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        (
            Self::triangle(self.phase),
            Self::triangle(self.phase + offset_cycles),
        )
    }
}

// ── Allpass primitives ──────────────────────────────────────────────────────

#[derive(Default, Clone, Copy)]
struct AllpassSection {
    a: f32,
    x1: f32,
    y1: f32,
}

impl AllpassSection {
    #[inline]
    fn set_coeff(&mut self, a: f32) {
        self.a = a;
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let y = self.a * (x - self.y1) + self.x1;
        self.x1 = x;
        self.y1 = y;
        y
    }
}

/// Bilinear-transform allpass coefficient: `a = (t − 1)/(t + 1)` with
/// `t = tan(π·fc/fs)`. `fc` clamped to a stable band.
#[inline]
fn allpass_coeff(break_hz: f32, sample_rate: f32) -> f32 {
    let norm = (break_hz / sample_rate).clamp(1.0e-4, 0.49);
    let t = (std::f32::consts::PI * norm).tan();
    (t - 1.0) / (t + 1.0)
}

/// Log sweep of ±2 octaves around `center_hz` at full `depth`, clamped to a
/// stable band.
#[inline]
fn swept_fc(center_hz: f32, depth: f32, tri: f32, nyquist_guard: f32) -> f32 {
    (center_hz * (2.0_f32).powf(2.0 * depth * tri)).clamp(40.0, nyquist_guard)
}

// ── Per-channel cascade ─────────────────────────────────────────────────────

#[derive(Clone)]
struct PhaserChannel {
    sample_rate: f32,
    sections: [AllpassSection; STAGES],
    stage_ratio: [f32; STAGES],
    a_cur: [f32; STAGES],
    a_step: [f32; STAGES],
    ramp_remaining: u32,
    fb_state: f32,
}

impl PhaserChannel {
    fn new(sample_rate: f32, seed: u64) -> Self {
        let mut s = seed | 1;
        let mut stage_ratio = [1.0_f32; STAGES];
        for r in stage_ratio.iter_mut() {
            let u = xorshift64(&mut s); // [-1, 1]
            *r = 1.0 + STAGE_SPREAD * u;
        }
        Self {
            sample_rate,
            sections: [AllpassSection::default(); STAGES],
            stage_ratio,
            a_cur: [0.0; STAGES],
            a_step: [0.0; STAGES],
            ramp_remaining: 0,
            fb_state: 0.0,
        }
    }

    /// Snap coefficients to `fc` with no ramp. Used at construction so the
    /// first block doesn't ramp up from `a = 0` (which would pass the dry
    /// signal unphased for one interval).
    fn snap(&mut self, fc: f32) {
        for i in 0..STAGES {
            let a = allpass_coeff(fc * self.stage_ratio[i], self.sample_rate);
            self.a_cur[i] = a;
            self.sections[i].set_coeff(a);
        }
        self.ramp_remaining = 0;
    }

    /// Schedule a linear ramp of each stage's coefficient toward the target
    /// for break frequency `fc`, over [`CONTROL_INTERVAL`] samples.
    fn schedule(&mut self, fc: f32) {
        let inv = 1.0 / CONTROL_INTERVAL as f32;
        for i in 0..STAGES {
            let target = allpass_coeff(fc * self.stage_ratio[i], self.sample_rate);
            self.a_step[i] = (target - self.a_cur[i]) * inv;
        }
        self.ramp_remaining = CONTROL_INTERVAL;
    }

    #[inline]
    fn advance_ramp(&mut self) {
        if self.ramp_remaining > 0 {
            for i in 0..STAGES {
                self.a_cur[i] += self.a_step[i];
                self.sections[i].set_coeff(self.a_cur[i]);
            }
            self.ramp_remaining -= 1;
        }
    }

    /// Run one sample through `x + soft(fb·feedback_state) → cascade`. `fb`
    /// is the already-clamped feedback amount.
    #[inline]
    fn process(&mut self, x: f32, fb: f32) -> f32 {
        let mut s = x + fast_tanh(fb * self.fb_state);
        for sect in self.sections.iter_mut() {
            s = sect.process(s);
        }
        self.fb_state = flush_denormal(s);
        s
    }

    fn clear(&mut self) {
        for sect in self.sections.iter_mut() {
            sect.x1 = 0.0;
            sect.y1 = 0.0;
        }
        self.fb_state = 0.0;
    }
}

// ── Stereo phaser ───────────────────────────────────────────────────────────

/// Stereo allpass phaser with anti-phase L/R LFO sweep. The collapsed macro
/// surface is **rate, depth, feedback, mix** — see the module docs for the
/// pinned structural defaults.
#[derive(Clone)]
pub struct StereoPhaser {
    sample_rate: f32,
    nyquist_guard: f32,
    left: PhaserChannel,
    right: PhaserChannel,
    lfo: TriangleLfo,
    control_counter: u32,
    rate_hz: f32,
    depth: f32,
    feedback: f32,
    mix: f32,
}

impl StereoPhaser {
    pub fn new(sample_rate: f32) -> Self {
        // Seeds chosen as in the upstream; the right channel gets a golden-
        // ratio XOR so the two channels' stage-stagger walks decorrelate.
        let mut left = PhaserChannel::new(sample_rate, 0x1F2E_3D4C);
        let mut right = PhaserChannel::new(sample_rate, 0x1F2E_3D4C ^ 0x9E37_79B9);
        left.snap(CENTER_HZ);
        right.snap(CENTER_HZ);
        Self {
            sample_rate,
            nyquist_guard: sample_rate * 0.45,
            left,
            right,
            lfo: TriangleLfo::new(),
            control_counter: 0,
            rate_hz: 0.5,
            depth: 0.7,
            feedback: 0.0,
            mix: 0.5,
        }
    }

    /// Empty the cascade and feedback memory; LFO phase reset to zero.
    pub fn clear(&mut self) {
        self.left.clear();
        self.right.clear();
        self.lfo.phase = 0.0;
        self.control_counter = 0;
        // Re-snap to the centre so the first sample after clear doesn't
        // ramp up from a = 0 (silent-cascade transient).
        self.left.snap(CENTER_HZ);
        self.right.snap(CENTER_HZ);
    }

    /// Set parameters for the next control block. `rate_hz` 0.05..10 Hz,
    /// `depth` and `mix` in `[0, 1]`, `feedback` in `[-0.9, 0.9]`.
    pub fn set_params(&mut self, rate_hz: f32, depth: f32, feedback: f32, mix: f32) {
        self.rate_hz = rate_hz.clamp(0.05, 10.0);
        self.depth = depth.clamp(0.0, 1.0);
        self.feedback = feedback.clamp(-FB_MAX, FB_MAX);
        self.mix = mix.clamp(0.0, 1.0);
        self.lfo.set_rate(self.rate_hz, self.sample_rate);
    }

    /// One stereo sample in / out. The L cascade reads the LFO at phase, the
    /// R cascade at phase + 0.5 (anti-phase).
    #[inline]
    pub fn process(&mut self, in_l: f32, in_r: f32) -> (f32, f32) {
        let (tri_l, tri_r) = self.lfo.tick_offset(0.5);

        if self.control_counter == 0 {
            let fc_l = swept_fc(CENTER_HZ, self.depth, tri_l, self.nyquist_guard);
            let fc_r = swept_fc(CENTER_HZ, self.depth, tri_r, self.nyquist_guard);
            self.left.schedule(fc_l);
            self.right.schedule(fc_r);
        }
        self.control_counter = (self.control_counter + 1) % CONTROL_INTERVAL;
        self.left.advance_ramp();
        self.right.advance_ramp();

        let wet_l = self.left.process(in_l, self.feedback);
        let wet_r = self.right.process(in_r, self.feedback);
        let dry_gain = 1.0 - self.mix;
        let wet_gain = self.mix;
        (
            dry_gain * in_l + wet_gain * wet_l,
            dry_gain * in_r + wet_gain * wet_r,
        )
    }

    /// Block process: mono `dry` in, stereo out. Pre-FX bus shape (matches
    /// [`StereoChorus::process_block`](crate::chorus::StereoChorus::process_block)).
    pub fn process_block(&mut self, dry: &[f32], out_l: &mut [f32], out_r: &mut [f32]) {
        let n = dry.len().min(out_l.len()).min(out_r.len());
        // Hoist what's constant across the block.
        let depth = self.depth;
        let feedback = self.feedback;
        let mix = self.mix;
        let dry_gain = 1.0 - mix;
        let nyq = self.nyquist_guard;

        for i in 0..n {
            let (tri_l, tri_r) = self.lfo.tick_offset(0.5);
            if self.control_counter == 0 {
                let fc_l = swept_fc(CENTER_HZ, depth, tri_l, nyq);
                let fc_r = swept_fc(CENTER_HZ, depth, tri_r, nyq);
                self.left.schedule(fc_l);
                self.right.schedule(fc_r);
            }
            self.control_counter = (self.control_counter + 1) % CONTROL_INTERVAL;
            self.left.advance_ramp();
            self.right.advance_ramp();

            let x = dry[i];
            let wet_l = self.left.process(x, feedback);
            let wet_r = self.right.process(x, feedback);
            out_l[i] = dry_gain * x + mix * wet_l;
            out_r[i] = dry_gain * x + mix * wet_r;
        }
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::lookup_sine;
    use crate::CONTROL_BLOCK;
    use std::f32::consts::TAU;

    const SR: f32 = 48_000.0;

    #[test]
    fn silent_input_stays_silent() {
        crate::enable_flush_to_zero();
        let mut ph = StereoPhaser::new(SR);
        ph.set_params(0.7, 1.0, 0.8, 1.0);
        for _ in 0..((SR * 0.2) as usize) {
            let (l, r) = ph.process(0.0, 0.0);
            assert!(l.is_finite() && r.is_finite());
            assert!(l.abs() < 1.0e-6 && r.abs() < 1.0e-6, "l={l} r={r}");
        }
    }

    #[test]
    fn mix_zero_is_identity() {
        crate::enable_flush_to_zero();
        let mut ph = StereoPhaser::new(SR);
        ph.set_params(0.7, 0.9, 0.6, 0.0);
        for i in 0..1_000 {
            let x = 0.4 * lookup_sine((i as f32 * 220.0 / SR).fract());
            let (l, r) = ph.process(x, -x);
            assert!((l - x).abs() < 1.0e-6, "L not pass-through: {l} vs {x}");
            assert!((r - (-x)).abs() < 1.0e-6, "R not pass-through: {r} vs {}", -x);
        }
    }

    #[test]
    fn stable_at_high_feedback() {
        crate::enable_flush_to_zero();
        let mut ph = StereoPhaser::new(SR);
        ph.set_params(0.7, 1.0, 0.85, 0.5);
        for i in 0..((SR * 10.0) as usize) {
            let t = i as f32 / SR;
            let x = 0.3 * (TAU * 220.0 * t).sin();
            let (l, r) = ph.process(x, x);
            assert!(l.is_finite() && r.is_finite(), "non-finite at i={i}");
            // 0.9 fb + cascade gain shouldn't run away past a sane envelope.
            assert!(l.abs() < 10.0 && r.abs() < 10.0, "runaway at i={i}: {l} {r}");
        }
    }

    #[test]
    fn depth_modulates_output() {
        // depth=0 freezes the sweep; depth=1 sweeps full ±2 oct. The two
        // configurations must produce different outputs for the same input.
        crate::enable_flush_to_zero();
        let mut a = StereoPhaser::new(SR);
        let mut b = StereoPhaser::new(SR);
        a.set_params(1.5, 0.0, 0.3, 0.7);
        b.set_params(1.5, 1.0, 0.3, 0.7);
        let mut diverged = false;
        for i in 0..((SR * 0.5) as usize) {
            let t = i as f32 / SR;
            let x = 0.3 * (TAU * 440.0 * t).sin();
            let (la, _) = a.process(x, x);
            let (lb, _) = b.process(x, x);
            if (la - lb).abs() > 1.0e-3 {
                diverged = true;
                break;
            }
        }
        assert!(diverged, "depth=1 vs depth=0 should produce audibly different output");
    }

    #[test]
    fn block_matches_per_sample() {
        // The block path and the per-sample wrapper share the same core; with
        // a mono source (l == r) they must agree sample-for-sample.
        crate::enable_flush_to_zero();
        let mut a = StereoPhaser::new(SR);
        let mut b = StereoPhaser::new(SR);
        a.set_params(0.6, 0.7, 0.3, 0.5);
        b.set_params(0.6, 0.7, 0.3, 0.5);

        let mut dry = [0.0f32; CONTROL_BLOCK];
        let mut bl = [0.0f32; CONTROL_BLOCK];
        let mut br = [0.0f32; CONTROL_BLOCK];
        for blk in 0..64 {
            for (i, d) in dry.iter_mut().enumerate() {
                let phase = ((blk * CONTROL_BLOCK + i) as f32 * 330.0 / SR).fract();
                *d = lookup_sine(phase);
            }
            b.process_block(&dry, &mut bl, &mut br);
            for (i, &d) in dry.iter().enumerate() {
                let (l, r) = a.process(d, d);
                assert!(
                    (l - bl[i]).abs() < 1.0e-5,
                    "L mismatch blk{blk} i{i}: {l} vs {}",
                    bl[i]
                );
                assert!(
                    (r - br[i]).abs() < 1.0e-5,
                    "R mismatch blk{blk} i{i}: {r} vs {}",
                    br[i]
                );
            }
        }
    }

    #[test]
    fn stereo_decorrelates_on_mono_input() {
        // Anti-phase LFO means a mono source produces L ≠ R after a brief
        // settle. Correlation should be well under 1.0.
        crate::enable_flush_to_zero();
        let mut ph = StereoPhaser::new(SR);
        ph.set_params(1.0, 0.9, 0.0, 0.5);

        let n = (SR * 0.5) as usize;
        let settle = (SR * 0.05) as usize;
        let (mut l_buf, mut r_buf) = (Vec::with_capacity(n), Vec::with_capacity(n));
        for i in 0..(settle + n) {
            let t = i as f32 / SR;
            let x = 0.3 * (TAU * 440.0 * t).sin();
            let (lo, ro) = ph.process(x, x);
            if i >= settle {
                l_buf.push(lo);
                r_buf.push(ro);
            }
        }
        let ml = l_buf.iter().sum::<f32>() / l_buf.len() as f32;
        let mr = r_buf.iter().sum::<f32>() / r_buf.len() as f32;
        let (mut num, mut dl, mut dr) = (0.0_f32, 0.0_f32, 0.0_f32);
        for i in 0..l_buf.len() {
            let a = l_buf[i] - ml;
            let b = r_buf[i] - mr;
            num += a * b;
            dl += a * a;
            dr += b * b;
        }
        let corr = num / (dl * dr).sqrt();
        assert!(corr < 0.9, "L/R should decorrelate under anti-phase sweep: corr {corr}");
    }
}
