//! Stereo look-ahead brick-wall limiter (the terminal master stage, ADR 0002).
//!
//! Delays the signal by the look-ahead window and drives the gain from a sliding
//! peak over exactly the samples currently in that window. Because the gain for
//! the sample emerging now is `≤ threshold / max(window)` and that sample is one
//! of the windowed peaks, the output is provably `≤ threshold` — a hard ceiling,
//! no clipping. Channels are linked (shared gain) so the stereo image is
//! preserved. The look-ahead is reported to the host as latency (PDC).

pub struct Limiter {
    threshold: f32,
    lookahead: usize,
    dl_l: Vec<f32>,
    dl_r: Vec<f32>,
    /// Per-slot input peak — same samples as the delay line.
    peaks: Vec<f32>,
    pos: usize,
    gain: f32,
    release_coef: f32,
}

impl Limiter {
    /// `lookahead` samples of latency; `threshold` linear ceiling (e.g. 0.95).
    pub fn new(sample_rate: f32, lookahead: usize, threshold: f32) -> Self {
        let la = lookahead.max(1);
        Self {
            threshold: threshold.clamp(0.01, 1.0),
            lookahead: la,
            dl_l: vec![0.0; la],
            dl_r: vec![0.0; la],
            peaks: vec![0.0; la],
            pos: 0,
            gain: 1.0,
            // ~80 ms release.
            release_coef: 1.0 - (-1.0 / (0.08 * sample_rate)).exp(),
        }
    }

    /// Reported latency in samples (the look-ahead). Constant — safe to report
    /// once via the CLAP `latency` extension.
    pub fn latency(&self) -> u32 {
        self.lookahead as u32
    }

    pub fn reset(&mut self) {
        self.dl_l.iter_mut().for_each(|x| *x = 0.0);
        self.dl_r.iter_mut().for_each(|x| *x = 0.0);
        self.peaks.iter_mut().for_each(|x| *x = 0.0);
        self.pos = 0;
        self.gain = 1.0;
    }

    /// Limit a stereo block in place. Allocation-free. Output `|x| ≤ threshold`.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let n = l.len().min(r.len());
        for i in 0..n {
            let in_l = l[i];
            let in_r = r[i];

            // Emerging (delayed) sample.
            let out_l = self.dl_l[self.pos];
            let out_r = self.dl_r[self.pos];

            // Sliding peak over the window currently in the delay line (includes
            // the emerging sample) → instantaneous gain ceiling.
            let wmax = self.peaks.iter().copied().fold(0.0_f32, f32::max);
            let inst = self.threshold / wmax.max(self.threshold);

            // Instant attack (drop to the ceiling), slow release toward unity —
            // but never let release climb back above the current ceiling, or the
            // hard-ceiling guarantee breaks.
            if inst < self.gain {
                self.gain = inst;
            } else {
                self.gain = (self.gain + (1.0 - self.gain) * self.release_coef).min(inst);
            }

            l[i] = out_l * self.gain;
            r[i] = out_r * self.gain;

            // Insert the current sample.
            self.dl_l[self.pos] = in_l;
            self.dl_r[self.pos] = in_r;
            self.peaks[self.pos] = in_l.abs().max(in_r.abs());
            self.pos = (self.pos + 1) % self.lookahead;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_never_exceeds_threshold() {
        let mut lim = Limiter::new(48_000.0, 64, 0.9);
        // Hammer it with a loud signal (peaks at 4.0).
        let mut l: Vec<f32> = (0..4_800).map(|i| 4.0 * ((i as f32 * 0.05).sin())).collect();
        let mut r = l.clone();
        lim.process(&mut l, &mut r);
        // After the look-ahead settles, nothing exceeds the ceiling.
        let max = l[128..]
            .iter()
            .chain(r[128..].iter())
            .fold(0.0_f32, |m, &x| m.max(x.abs()));
        assert!(max <= 0.9 + 1e-4, "peak {max} exceeds threshold");
    }

    #[test]
    fn quiet_signal_passes_through() {
        let mut lim = Limiter::new(48_000.0, 64, 0.9);
        let input: Vec<f32> = (0..2_000).map(|i| 0.3 * (i as f32 * 0.03).sin()).collect();
        let mut l = input.clone();
        let mut r = input.clone();
        lim.process(&mut l, &mut r);
        // Same shape, delayed by the look-ahead, gain ~1.
        for i in 0..1_000 {
            assert!((l[i + 64] - input[i]).abs() < 1e-3, "quiet passes undistorted at {i}");
        }
    }

    #[test]
    fn reports_lookahead_latency() {
        let lim = Limiter::new(48_000.0, 64, 0.9);
        assert_eq!(lim.latency(), 64);
    }
}
