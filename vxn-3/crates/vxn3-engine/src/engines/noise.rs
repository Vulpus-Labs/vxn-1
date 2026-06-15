//! `Noise` — the noise-percussion engine. Lanes = **voices** (the poly branch,
//! ADR 0001 §5).
//!
//! A white-noise burst plus an optional short tuned body, each with its own
//! exponential decay, brightened by a one-pole output highpass. Independent
//! short tails → poly voicing (the simplest case, same shape as `Kick/Tone`).
//! Covers snare / clap / misc percussion.
//!
//! 4-wide SoA voice state in plain arrays (branchless envelopes) so the lane
//! loop autovectorises.

use vxn3_dsp::{SILENCE_EPS, decay_coef, fast_sine_q32, note_to_freq, phase_inc_hz};

use crate::track_engine::{EngineKind, Knob, LANES, TrackEngine};

#[derive(Copy, Clone, Debug)]
pub struct NoisePatch {
    /// Noise-burst decay to -60 dB (s).
    pub noise_decay_s: f32,
    /// Tuned-body decay to -60 dB (s).
    pub tone_decay_s: f32,
    /// Tuned-body mix 0..1 (0 = pure noise clap, ~0.4 = snare body).
    pub tone_mix: f32,
    /// Output highpass cutoff (Hz) — brightness.
    pub hp_hz: f32,
}

impl Default for NoisePatch {
    /// A serviceable snare.
    fn default() -> Self {
        Self {
            noise_decay_s: 0.18,
            tone_decay_s: 0.12,
            tone_mix: 0.35,
            hp_hz: 1_200.0,
        }
    }
}

pub struct Noise {
    patch: NoisePatch,
    sample_rate: f32,

    noise_decay: f32,
    tone_decay: f32,
    hp_coef: f32,

    rng: u32,
    // One-pole highpass state (engine output).
    hp_y: f32,
    hp_x1: f32,

    // ── per-voice SoA state ──
    noise_env: [f32; LANES],
    tone_env: [f32; LANES],
    tone_phase: [u32; LANES],
    tone_inc: [u32; LANES],
    peak: [f32; LANES],
    active: [bool; LANES],
    next: usize,
}

impl Noise {
    pub fn new(sample_rate: f32, patch: NoisePatch) -> Self {
        let mut e = Self {
            patch,
            sample_rate,
            noise_decay: 0.0,
            tone_decay: 0.0,
            hp_coef: 0.0,
            rng: 0x1234_5678,
            hp_y: 0.0,
            hp_x1: 0.0,
            noise_env: [0.0; LANES],
            tone_env: [0.0; LANES],
            tone_phase: [0; LANES],
            tone_inc: [0; LANES],
            peak: [0.0; LANES],
            active: [false; LANES],
            next: 0,
        };
        e.cook();
        e
    }

    pub fn with_default_patch(sample_rate: f32) -> Self {
        Self::new(sample_rate, NoisePatch::default())
    }

    fn cook(&mut self) {
        self.noise_decay = decay_coef(self.patch.noise_decay_s, self.sample_rate);
        self.tone_decay = decay_coef(self.patch.tone_decay_s, self.sample_rate);
        // One-pole highpass coefficient.
        self.hp_coef = (-std::f32::consts::TAU * self.patch.hp_hz / self.sample_rate).exp();
    }

    #[inline]
    fn white(&mut self) -> f32 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x as i32 as f32) * (1.0 / 2_147_483_648.0)
    }

    fn alloc_lane(&mut self) -> usize {
        if let Some(k) = (0..LANES).find(|&k| !self.active[k]) {
            return k;
        }
        let k = self.next;
        self.next = (self.next + 1) % LANES;
        k
    }
}

impl TrackEngine for Noise {
    fn render(&mut self, out: &mut [f32]) {
        let nd = self.noise_decay;
        let td = self.tone_decay;
        let mix = self.patch.tone_mix;
        let hp = self.hp_coef;

        for s in out.iter_mut() {
            let n = self.white();
            let mut acc = 0.0_f32;
            for k in 0..LANES {
                self.noise_env[k] *= nd;
                self.tone_env[k] *= td;
                self.tone_phase[k] = self.tone_phase[k].wrapping_add(self.tone_inc[k]);
                let tone = fast_sine_q32(self.tone_phase[k]) * self.tone_env[k];
                let noise = n * self.noise_env[k];
                acc += (noise * (1.0 - mix) + tone * mix) * self.peak[k];
            }
            // One-pole highpass: y = hp*(y + x - x1).
            let y = hp * (self.hp_y + acc - self.hp_x1);
            self.hp_y = y;
            self.hp_x1 = acc;
            *s = y;
        }

        for k in 0..LANES {
            if self.active[k] && self.noise_env[k] < SILENCE_EPS && self.tone_env[k] < SILENCE_EPS {
                self.active[k] = false;
            }
        }
    }

    fn on_trig(&mut self, note: f32, velocity: f32) {
        let k = self.alloc_lane();
        self.noise_env[k] = 1.0;
        self.tone_env[k] = 1.0;
        self.tone_phase[k] = 0;
        self.tone_inc[k] = phase_inc_hz(note_to_freq(note), self.sample_rate) as u32;
        self.peak[k] = velocity;
        self.active[k] = true;
    }

    fn reset(&mut self) {
        self.noise_env = [0.0; LANES];
        self.tone_env = [0.0; LANES];
        self.peak = [0.0; LANES];
        self.active = [false; LANES];
        self.hp_y = 0.0;
        self.hp_x1 = 0.0;
        self.next = 0;
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        self.cook();
    }

    fn kind(&self) -> EngineKind {
        EngineKind::Noise
    }

    fn set_knob(&mut self, knob: Knob, value: f32) {
        let v = value.clamp(0.0, 1.0);
        match knob {
            Knob::Tone => self.patch.tone_mix = v,                   // noise ↔ body
            Knob::Decay => self.patch.noise_decay_s = 0.02 + v * 0.48, // 20 ms .. 0.5 s
            Knob::Pitch => {}
        }
        self.cook();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rms(b: &[f32]) -> f32 {
        (b.iter().map(|&x| x * x).sum::<f32>() / b.len().max(1) as f32).sqrt()
    }

    #[test]
    fn idle_is_silent() {
        let mut e = Noise::with_default_patch(48_000.0);
        let mut buf = [1.0_f32; 256];
        e.render(&mut buf);
        // The highpass settles the dirtied buffer toward zero quickly.
        assert!(rms(&buf[64..]) < 1e-3, "idle → silence");
    }

    #[test]
    fn trig_produces_perc_then_decays() {
        let mut e = Noise::with_default_patch(48_000.0);
        e.on_trig(60.0, 1.0);
        let mut body = vec![0.0_f32; 2_400]; // 50 ms
        e.render(&mut body);
        assert!(rms(&body) > 0.02, "perc hit audible, rms={}", rms(&body));
        assert!(body.iter().all(|x| x.is_finite()));

        let mut decay = vec![0.0_f32; 24_000]; // 0.5 s ≫ the 0.18 s decay
        e.render(&mut decay);
        let mut tail = vec![0.0_f32; 9_600];
        e.render(&mut tail);
        assert!(rms(&tail) < 1e-4, "fully decayed, rms={}", rms(&tail));
        assert!(!e.active.iter().any(|&a| a), "lane freed");
    }

    #[test]
    fn voices_are_independent() {
        let mut e = Noise::with_default_patch(48_000.0);
        for _ in 0..LANES {
            e.on_trig(60.0, 1.0);
        }
        assert_eq!(e.active.iter().filter(|&&a| a).count(), LANES);
    }
}
