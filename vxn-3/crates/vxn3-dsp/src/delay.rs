//! Stereo send delay (the dub bus, ADR 0002 / 0001 §3).
//!
//! Ping-pong cross-feedback for a dubby stereo image, with a `tanh` saturator in
//! the feedback path so feedback **past unity self-oscillates but stays bounded**
//! (a tape/BBD-ish runaway that doesn't blow up). Fully wet: the bus *is* the
//! send path; the return level into the master mix is the wet amount. Buffers are
//! pre-allocated to a max delay; `process` never allocates.

pub struct Delay {
    buf_l: Vec<f32>,
    buf_r: Vec<f32>,
    cap: usize,
    write: usize,
    delay_samples: usize,
    feedback: f32,
    /// One-pole damping in the feedback path (tape/BBD high-cut).
    damp: f32,
    lp_l: f32,
    lp_r: f32,
}

impl Delay {
    /// Allocate for up to `max_seconds` of delay at `sample_rate`.
    pub fn new(sample_rate: f32, max_seconds: f32) -> Self {
        let cap = ((sample_rate * max_seconds) as usize).max(2);
        Self {
            buf_l: vec![0.0; cap],
            buf_r: vec![0.0; cap],
            cap,
            write: 0,
            delay_samples: cap / 4,
            feedback: 0.5,
            damp: 0.3,
            lp_l: 0.0,
            lp_r: 0.0,
        }
    }

    /// Set the delay time in samples, clamped to the allocated buffer.
    pub fn set_delay_samples(&mut self, n: usize) {
        self.delay_samples = n.clamp(1, self.cap - 1);
    }

    /// Feedback amount; values `> 1` self-oscillate (bounded by the saturator).
    pub fn set_feedback(&mut self, fb: f32) {
        self.feedback = fb.clamp(0.0, 1.3);
    }

    /// Feedback-path damping 0..1 (higher = darker repeats).
    pub fn set_damp(&mut self, d: f32) {
        self.damp = d.clamp(0.0, 0.99);
    }

    pub fn reset(&mut self) {
        self.buf_l.iter_mut().for_each(|x| *x = 0.0);
        self.buf_r.iter_mut().for_each(|x| *x = 0.0);
        self.lp_l = 0.0;
        self.lp_r = 0.0;
        self.write = 0;
    }

    /// Process a stereo send block into the wet `out_*` buffers (overwrites).
    /// Allocation-free.
    pub fn process(&mut self, in_l: &[f32], in_r: &[f32], out_l: &mut [f32], out_r: &mut [f32]) {
        let n = in_l.len().min(in_r.len()).min(out_l.len()).min(out_r.len());
        for i in 0..n {
            let read = (self.write + self.cap - self.delay_samples) % self.cap;
            let wet_l = self.buf_l[read];
            let wet_r = self.buf_r[read];
            out_l[i] = wet_l;
            out_r[i] = wet_r;

            // Damp the feedback signal (one-pole low-pass).
            self.lp_l += (wet_l - self.lp_l) * (1.0 - self.damp);
            self.lp_r += (wet_r - self.lp_r) * (1.0 - self.damp);

            // Ping-pong: each side feeds the other, saturated so fb > 1 stays
            // bounded (controllable self-oscillation).
            self.buf_l[self.write] = in_l[i] + (self.feedback * self.lp_r).tanh();
            self.buf_r[self.write] = in_r[i] + (self.feedback * self.lp_l).tanh();
            self.write = (self.write + 1) % self.cap;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rms(b: &[f32]) -> f32 {
        (b.iter().map(|&x| x * x).sum::<f32>() / b.len().max(1) as f32).sqrt()
    }

    #[test]
    fn impulse_echoes_after_delay_time() {
        let mut d = Delay::new(48_000.0, 1.0);
        d.set_delay_samples(100);
        d.set_feedback(0.0);
        let mut il = vec![0.0_f32; 300];
        let mut ir = vec![0.0_f32; 300];
        il[0] = 1.0;
        ir[0] = 1.0;
        let mut ol = vec![0.0_f32; 300];
        let mut or_ = vec![0.0_f32; 300];
        d.process(&il, &ir, &mut ol, &mut or_);
        // The impulse re-appears ~100 samples later (ping-pong: L impulse → R out).
        assert!(ol[..50].iter().all(|&x| x.abs() < 1e-6), "no early echo");
        assert!(or_[100].abs() > 0.5 || ol[100].abs() > 0.5, "echo at delay time");
    }

    #[test]
    fn feedback_past_unity_self_oscillates_bounded() {
        let mut d = Delay::new(48_000.0, 1.0);
        d.set_delay_samples(200);
        d.set_feedback(1.15); // past unity
        let mut il = vec![0.0_f32; 48_000];
        let mut ir = vec![0.0_f32; 48_000];
        il[0] = 1.0; // single kick into the bus, then silence
        ir[0] = 1.0;
        let mut ol = vec![0.0_f32; 48_000];
        let mut or_ = vec![0.0_f32; 48_000];
        d.process(&il, &ir, &mut ol, &mut or_);
        // Still ringing 1 s later (self-oscillating), but bounded + finite.
        let tail = &ol[40_000..];
        assert!(rms(tail) > 0.01, "self-oscillates, tail rms={}", rms(tail));
        assert!(
            ol.iter().chain(or_.iter()).all(|&x| x.is_finite() && x.abs() < 4.0),
            "bounded by the saturator"
        );
    }
}
