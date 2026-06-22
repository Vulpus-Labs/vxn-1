//! Circular delay line and a stereo feedback delay effect. The interpolation
//! technique follows `patches-dsp::delay_buffer` (power-of-two mask + linear
//! read); the stereo wrapper is VXN1's own.

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
    delay_samples_l: f32,
    delay_samples_r: f32,
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
        self.delay_samples_l = (time_l * self.sample_rate).clamp(1.0, max);
        self.delay_samples_r = (time_r * self.sample_rate).clamp(1.0, max);
        self.feedback = feedback.clamp(0.0, 0.99);
        self.damping = damping.clamp(0.0, 1.0);
        self.mix = mix.clamp(0.0, 1.0);
    }

    /// Process one stereo sample.
    #[inline]
    pub fn process(&mut self, in_l: f32, in_r: f32) -> (f32, f32) {
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
            d.delay_samples_l <= max,
            "left not clamped: {}",
            d.delay_samples_l
        );
        assert!(
            d.delay_samples_r <= max,
            "right not clamped: {}",
            d.delay_samples_r
        );
        // Still produces finite output at the clamped time.
        for i in 0..sr as usize {
            let x = if i == 0 { 1.0 } else { 0.0 };
            let (l, r) = d.process(x, x);
            assert!(l.is_finite() && r.is_finite());
        }
    }
}
