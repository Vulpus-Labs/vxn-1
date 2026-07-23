//! Pre-FX cleanup filters.
//!
//! Stereo one-pole HPF (20 Hz) → one-pole LPF (18 kHz) on the post-sum bus,
//! between the stack mix and the delay input. Fixed cutoffs, no setters,
//! no user controls.
//!
//! ## Forms
//!
//! - HPF: `y = x - x_prev + a * y_prev`. The leading `(1 - z^-1)` places a
//!   zero at DC, so any non-zero cutoff drives DC gain to exactly 0 —
//!   subsuming a separate DC blocker.
//! - LPF: `y = b * x + (1 - b) * y_prev`. Standard one-pole low-pass.
//!
//! Coefficients are baked at construction from the prepared sample rate;
//! a sample-rate change is handled by the host calling `Engine::new` /
//! `prepare_to_play` again, which builds a fresh filter.
//!
//! ## Why pre-FX, not per-voice
//!
//! 16 stacks × 8 lanes = 128 lanes. A per-lane cleanup filter would tick
//! 128× per sample for no audible benefit — the rumble / ultrasonic content
//! is the same whether you remove it once on the bus or 128 times on the
//! lanes. Pre-FX is one stereo stage, four mul-adds total per sample.
//!
//! ## Why pre-delay, not post-master
//!
//! Sub-rumble and 20 kHz sidebands fed into the delay feedback path or
//! reverb FDN compound on every tap / iteration. Cleanup belongs upstream
//! of the spatial FX so they never see the offending content.

use core::f32::consts::TAU;

/// Highpass cutoff (Hz). Below human hearing — strips DC offset and
/// sub-audible rumble without touching the bass register.
const HPF_FC_HZ: f32 = 20.0;
/// Lowpass cutoff (Hz). Above the practical audible band — gently rolls
/// off ultrasonic content that would alias on delay re-reads / FDN taps.
const LPF_FC_HZ: f32 = 18_000.0;

/// Per-channel filter state. One [`OneChannel`] per stereo side.
#[derive(Default, Clone, Copy)]
struct OneChannel {
    /// `x[n-1]` — needed for the HPF zero at DC.
    x_prev: f32,
    /// `y_hp[n-1]` — HPF feedback state.
    y_hp_prev: f32,
    /// `y_lp[n-1]` — LPF feedback state.
    y_lp_prev: f32,
}

/// Stereo cleanup chain: HPF then LPF, per side, with shared coefficients.
pub struct CleanupFilter {
    a_hp: f32,
    b_lp: f32,
    l: OneChannel,
    r: OneChannel,
}

impl CleanupFilter {
    /// Build a fresh filter for `sample_rate`. Coefficients are baked once
    /// here; there is no runtime sample-rate setter.
    pub fn new(sample_rate: f32) -> Self {
        let a_hp = (-TAU * HPF_FC_HZ / sample_rate).exp();
        let b_lp = 1.0 - (-TAU * LPF_FC_HZ / sample_rate).exp();
        Self {
            a_hp,
            b_lp,
            l: OneChannel::default(),
            r: OneChannel::default(),
        }
    }

    /// Clear both channels' state. Called from `Engine::reset()` alongside
    /// the delay / reverb tail wipes.
    pub fn reset(&mut self) {
        self.l = OneChannel::default();
        self.r = OneChannel::default();
    }

    #[inline]
    fn step(a_hp: f32, b_lp: f32, ch: &mut OneChannel, x: f32) -> f32 {
        let y_hp = x - ch.x_prev + a_hp * ch.y_hp_prev;
        ch.x_prev = x;
        ch.y_hp_prev = y_hp;
        let y_lp = b_lp * y_hp + (1.0 - b_lp) * ch.y_lp_prev;
        ch.y_lp_prev = y_lp;
        y_lp
    }

    #[inline]
    pub fn process(&mut self, in_l: f32, in_r: f32) -> (f32, f32) {
        let l = Self::step(self.a_hp, self.b_lp, &mut self.l, in_l);
        let r = Self::step(self.a_hp, self.b_lp, &mut self.r, in_r);
        (l, r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    /// DC removal: feed a constant 1.0 for 1 s; the last 100 ms of output
    /// must be at zero within 1e-5. The HPF's zero at z=1 makes this exact
    /// in steady state.
    #[test]
    fn dc_removed_to_floor() {
        let mut f = CleanupFilter::new(SR);
        let n_total = SR as usize;
        let n_tail = (SR * 0.1) as usize;
        let mut tail_sum = 0.0_f64;
        for i in 0..n_total {
            let (l, r) = f.process(1.0, 1.0);
            if i >= n_total - n_tail {
                tail_sum += (l + r) as f64;
            }
        }
        let mean = tail_sum / (2.0 * n_tail as f64);
        assert!(mean.abs() < 1.0e-5, "DC tail mean {mean} not within 1e-5");
    }

    /// Passband flatness: a 100 Hz sine at 0 dBFS must lose less than
    /// 0.5 dB through the chain (HPF is ~0.2 dB down at 100 Hz; LPF is
    /// flat in the midband).
    #[test]
    fn midband_passband_flat() {
        let mut f = CleanupFilter::new(SR);
        let freq = 100.0_f32;
        let n = SR as usize;
        let mut in_sq = 0.0_f64;
        let mut out_sq = 0.0_f64;
        // Skip the first 100 ms so the HPF transient (τ ≈ 8 ms) is gone
        // before we measure.
        let warmup = (SR * 0.1) as usize;
        for i in 0..n {
            let t = i as f32 / SR;
            let x = (TAU * freq * t).sin();
            let (l, _) = f.process(x, x);
            if i >= warmup {
                in_sq += (x as f64).powi(2);
                out_sq += (l as f64).powi(2);
            }
        }
        let rms_in = (in_sq / (n - warmup) as f64).sqrt();
        let rms_out = (out_sq / (n - warmup) as f64).sqrt();
        let db = 20.0 * (rms_out / rms_in).log10();
        assert!(db.abs() < 0.5, "100 Hz attenuation {db} dB outside ±0.5");
    }

    /// Reset wipes state — silence after reset must be exact.
    #[test]
    fn reset_clears_state() {
        let mut f = CleanupFilter::new(SR);
        for _ in 0..1000 {
            let _ = f.process(0.7, -0.3);
        }
        f.reset();
        let (l, r) = f.process(0.0, 0.0);
        assert_eq!(l, 0.0);
        assert_eq!(r, 0.0);
    }
}
