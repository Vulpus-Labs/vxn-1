//! Circular delay line and a stereo feedback delay effect. The interpolation
//! technique follows `patches-dsp::delay_buffer` (power-of-two mask + linear
//! read); the stereo wrapper is VXN1's own.

use crate::one_pole_coeff;

/// Delay-time slew time constant (ms). The control layer hands the delay a
/// stepped target each block; the read pointer slews toward it *per sample*
/// (0015) so a DelayTime automation move bends the pitch like a tape/BBD line
/// instead of clicking. This is why `GlobalParam::DelayTime` snaps in the
/// engine smoother — its ramp lives here, the same way cutoff/reso ramp inside
/// the ladder rather than in `ParamSmoother`.
const TIME_SLEW_MS: f32 = 40.0;

/// Power-of-two circular buffer with fractional (linear-interpolated) reads.
#[derive(Clone)]
pub struct DelayLine {
    buf: Vec<f32>,
    mask: usize,
    write: usize,
}

impl DelayLine {
    /// Allocate a line that can hold at least `max_samples` of delay. Capacity
    /// is rounded up to a power of two. Allocates — call off the audio thread.
    pub fn new(max_samples: usize) -> Self {
        let cap = max_samples.max(2).next_power_of_two();
        Self {
            buf: vec![0.0; cap],
            mask: cap - 1,
            write: 0,
        }
    }

    pub fn clear(&mut self) {
        self.buf.iter_mut().for_each(|s| *s = 0.0);
        self.write = 0;
    }

    pub fn capacity(&self) -> usize {
        self.buf.len()
    }

    /// Write one sample at the current head and advance.
    #[inline]
    pub fn write(&mut self, x: f32) {
        self.buf[self.write] = x;
        self.write = (self.write + 1) & self.mask;
    }

    /// Read `delay_samples` (fractional) behind the most recent write.
    #[inline]
    pub fn read(&self, delay_samples: f32) -> f32 {
        let d = delay_samples.clamp(1.0, (self.buf.len() - 2) as f32);
        // `write` already points one past the last written sample.
        let read_pos = self.write as f32 - 1.0 - d;
        let read_pos = read_pos + self.buf.len() as f32; // keep positive
        let i = read_pos as usize;
        let frac = read_pos - i as f32;
        let a = self.buf[i & self.mask];
        let b = self.buf[(i + 1) & self.mask];
        a + (b - a) * frac
    }
}

/// Stereo feedback delay with a one-pole HF damping in the feedback path and a
/// dry/wet mix. Feedback always cross-feeds (ping-pong) between channels.
#[derive(Clone)]
pub struct StereoDelay {
    sample_rate: f32,
    left: DelayLine,
    right: DelayLine,
    fb_lp_l: f32,
    fb_lp_r: f32,
    // Control-block parameters.
    // `delay_samples_*` is the *current* read distance, slewed per sample toward
    // `target_samples_*` (set once per block) so a DelayTime move never jumps the
    // pointer (0015). `feedback`/`damping`/`mix` are smoothed upstream by the
    // engine's block-rate `ParamSmoother`, so they snap here.
    delay_samples_l: f32,
    delay_samples_r: f32,
    target_samples_l: f32,
    target_samples_r: f32,
    time_slew: f32,
    feedback: f32,
    damping: f32,
    mix: f32,
}

impl StereoDelay {
    pub fn new(sample_rate: f32, max_seconds: f32) -> Self {
        let max = (sample_rate * max_seconds) as usize + 4;
        Self {
            sample_rate,
            left: DelayLine::new(max),
            right: DelayLine::new(max),
            fb_lp_l: 0.0,
            fb_lp_r: 0.0,
            delay_samples_l: sample_rate * 0.3,
            delay_samples_r: sample_rate * 0.3,
            target_samples_l: sample_rate * 0.3,
            target_samples_r: sample_rate * 0.3,
            time_slew: one_pole_coeff(TIME_SLEW_MS, sample_rate),
            feedback: 0.4,
            damping: 0.3,
            mix: 0.25,
        }
    }

    pub fn clear(&mut self) {
        self.left.clear();
        self.right.clear();
        self.fb_lp_l = 0.0;
        self.fb_lp_r = 0.0;
        // Snap the read pointer to its target so a reset / preset load doesn't
        // glide the (now-empty) line from the previous patch's time.
        self.delay_samples_l = self.target_samples_l;
        self.delay_samples_r = self.target_samples_r;
    }

    /// Set parameters for the next control block. `time_l/time_r` in seconds,
    /// `feedback`/`mix`/`damping` in `[0, 1]`.
    pub fn set_params(
        &mut self,
        time_l: f32,
        time_r: f32,
        feedback: f32,
        damping: f32,
        mix: f32,
    ) {
        // Clamp to the buffer's usable span (its `read` clamps the same way).
        // A tempo-synced time at a slow subdivision/tempo can exceed capacity;
        // pin it here so the delay never wraps past its allocation.
        let max = (self.left.capacity().min(self.right.capacity()) - 2) as f32;
        self.target_samples_l = (time_l * self.sample_rate).clamp(1.0, max);
        self.target_samples_r = (time_r * self.sample_rate).clamp(1.0, max);
        self.feedback = feedback.clamp(0.0, 0.99);
        self.damping = damping.clamp(0.0, 1.0);
        self.mix = mix.clamp(0.0, 1.0);
    }

    /// Process one stereo sample.
    #[inline]
    pub fn process(&mut self, in_l: f32, in_r: f32) -> (f32, f32) {
        // Slew the read distance toward its target per sample so an automated
        // DelayTime sweep bends the pitch continuously (no pointer jump / click).
        self.delay_samples_l += self.time_slew * (self.target_samples_l - self.delay_samples_l);
        self.delay_samples_r += self.time_slew * (self.target_samples_r - self.delay_samples_r);
        let wet_l = self.left.read(self.delay_samples_l);
        let wet_r = self.right.read(self.delay_samples_r);

        // HF damping in the feedback path (one-pole lowpass).
        let a = self.damping;
        self.fb_lp_l = self.fb_lp_l + (1.0 - a) * (wet_l - self.fb_lp_l);
        self.fb_lp_r = self.fb_lp_r + (1.0 - a) * (wet_r - self.fb_lp_r);

        let fb_l = self.fb_lp_l * self.feedback;
        let fb_r = self.fb_lp_r * self.feedback;

        self.left.write(in_l + fb_r);
        self.right.write(in_r + fb_l);

        // Equal-power crossfade: the delayed wet is decorrelated from dry, so
        // sqrt gains hold total power constant across the sweep (linear gains
        // dip ~3 dB at mix=0.5).
        let m = self.mix;
        let dry = (1.0 - m).sqrt();
        let wet = m.sqrt();
        (in_l * dry + wet_l * wet, in_r * dry + wet_r * wet)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn impulse_reappears_after_delay() {
        let mut d = DelayLine::new(1000);
        d.write(1.0);
        for _ in 0..99 {
            d.write(0.0);
        }
        // Wrote 100 samples; the impulse sits 99 behind the most recent write.
        assert!((d.read(99.0) - 1.0).abs() < 1e-4, "got {}", d.read(99.0));
    }

    #[test]
    fn stereo_delay_decays_and_is_finite() {
        let sr = 48_000.0;
        let mut d = StereoDelay::new(sr, 2.0);
        d.set_params(0.01, 0.01, 0.5, 0.3, 1.0);
        let mut peak = 0.0f32;
        for i in 0..sr as usize {
            let x = if i == 0 { 1.0 } else { 0.0 };
            let (l, r) = d.process(x, x);
            assert!(l.is_finite() && r.is_finite());
            peak = peak.max(l.abs());
        }
        assert!(peak < 5.0, "delay unstable: {peak}");
    }

    #[test]
    fn delay_time_clamped_to_buffer_capacity() {
        let sr = 48_000.0;
        let mut d = StereoDelay::new(sr, 2.0);
        // Ask for 60 s (e.g. 1/1 at a very slow synced tempo) — far past the
        // 2 s buffer. It must clamp to the line's usable span, not wrap.
        d.set_params(60.0, 60.0, 0.5, 0.3, 1.0);
        let max = (d.left.capacity() - 2) as f32;
        assert!(
            d.target_samples_l <= max,
            "left not clamped: {}",
            d.target_samples_l
        );
        assert!(
            d.target_samples_r <= max,
            "right not clamped: {}",
            d.target_samples_r
        );
        // Still produces finite output at the clamped time.
        for i in 0..sr as usize {
            let x = if i == 0 { 1.0 } else { 0.0 };
            let (l, r) = d.process(x, x);
            assert!(l.is_finite() && r.is_finite());
        }
    }

    #[test]
    fn delay_time_sweep_is_click_free() {
        // 0015: a DelayTime automation sweep must not jump the read pointer. Run
        // the identical sweep through the real (per-sample slewed) delay and a
        // reference copy that snaps the pointer to its target each block (the old
        // behaviour), and assert the slewed wet output's worst sample-to-sample
        // step is far smaller. Self-calibrating — no magic threshold.
        let sr = 48_000.0;

        // Pure tone source, regenerated identically for both runs.
        let tone = |n: usize| {
            let dphase = 2.0 * std::f32::consts::PI * 220.0 / sr;
            (n as f32 * dphase).sin()
        };

        // One run; `snap` forces the pointer to its target each block.
        let run = |snap: bool| -> f32 {
            let mut d = StereoDelay::new(sr, 2.0);
            d.set_params(0.30, 0.30, 0.0, 0.0, 1.0); // wet-only, no feedback
            d.clear();
            let mut n = 0usize;
            // Prime the line.
            for _ in 0..(sr as usize) {
                d.process(tone(n), tone(n));
                n += 1;
            }
            // Gentle, realistic sweep: 0.30 s -> 0.10 s over ~1 s, stepped once
            // per 32-sample control block.
            let blocks = 1500;
            let mut worst = 0.0f32;
            let mut prev = d.process(tone(n), tone(n)).0;
            n += 1;
            for b in 0..blocks {
                let t = 0.30 + (0.10 - 0.30) * (b as f32 / blocks as f32);
                d.set_params(t, t, 0.0, 0.0, 1.0);
                if snap {
                    d.delay_samples_l = d.target_samples_l;
                    d.delay_samples_r = d.target_samples_r;
                }
                for _ in 0..32 {
                    let cur = d.process(tone(n), tone(n)).0;
                    n += 1;
                    assert!(cur.is_finite());
                    worst = worst.max((cur - prev).abs());
                    prev = cur;
                }
            }
            worst
        };

        let slewed = run(false);
        let snapped = run(true);
        assert!(
            slewed < 0.5 * snapped,
            "DelayTime slew not smoothing the sweep: slewed {slewed} vs snapped {snapped}"
        );
    }
}
