//! Parameter smoothing. One-pole smoother removes zipper noise when a host
//! parameter — or a mod-matrix destination accumulator — jumps. Update the
//! target once per control block; tick once per sample.
//!
//! Lifted from VXN1's `vxn-dsp::smoothing` so the mod-matrix (ticket 0008) can
//! apply per-sample smoothing on pitch-shaped destinations without depending
//! on the sibling workspace.

/// Samples for a given duration in milliseconds.
#[inline]
pub fn ms_to_samples(ms: f32, sample_rate: f32) -> usize {
    (ms * 0.001 * sample_rate).max(0.0) as usize
}

/// One-pole smoothing coefficient: `1 - exp(-1 / (ms * 0.001 * sr))`. Applied
/// as `y += coeff * (target - y)`. Larger `ms` → slower glide.
#[inline]
pub fn one_pole_coeff(ms: f32, sample_rate: f32) -> f32 {
    let n = (ms * 0.001 * sample_rate).max(1.0);
    1.0 - (-1.0 / n).exp()
}

/// Distance below which the glide snaps to its target instead of crawling
/// down the one-pole's asymptotic tail forever — a residual scaled by a wide
/// pitch depth is an audible offset that lingers after the source is released.
const SNAP_EPS: f32 = 1.0e-6;

/// A smoothed scalar parameter.
#[derive(Clone, Copy, Debug)]
pub struct Smoothed {
    current: f32,
    target: f32,
    coeff: f32,
}

impl Smoothed {
    pub fn new(initial: f32, ms: f32, sample_rate: f32) -> Self {
        Self {
            current: initial,
            target: initial,
            coeff: one_pole_coeff(ms, sample_rate),
        }
    }

    pub fn set_time(&mut self, ms: f32, sample_rate: f32) {
        self.coeff = one_pole_coeff(ms, sample_rate);
    }

    #[inline]
    pub fn set_target(&mut self, target: f32) {
        self.target = target;
    }

    pub fn snap(&mut self, value: f32) {
        self.current = value;
        self.target = value;
    }

    #[inline]
    pub fn tick(&mut self) -> f32 {
        self.current += self.coeff * (self.target - self.current);
        if (self.target - self.current).abs() < SNAP_EPS {
            self.current = self.target;
        }
        self.current
    }

    #[inline]
    pub fn current(&self) -> f32 {
        self.current
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converges_to_target() {
        let sr = 48_000.0;
        let mut s = Smoothed::new(0.0, 5.0, sr);
        s.set_target(1.0);
        for _ in 0..(sr as usize / 10) {
            s.tick();
        }
        assert!((s.current() - 1.0).abs() < 1e-3, "got {}", s.current());
    }

    #[test]
    fn snap_is_immediate() {
        let mut s = Smoothed::new(0.0, 100.0, 48_000.0);
        s.snap(0.5);
        assert_eq!(s.tick(), 0.5);
    }

    #[test]
    fn settles_exactly_to_target() {
        let mut s = Smoothed::new(1.0, 20.0, 1_500.0);
        s.set_target(0.0);
        let mut ticks = 0;
        while s.current() != 0.0 {
            s.tick();
            ticks += 1;
            assert!(ticks < 10_000, "never reached exactly 0.0");
        }
    }
}
