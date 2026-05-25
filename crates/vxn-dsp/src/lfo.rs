//! Low-frequency oscillator. Adapted from `patches-modules::modulators::lfo`,
//! collapsed to a single selectable shape and a free-running phase.
//!
//! Output is bipolar `[-1, 1]`. No BLEP (LFO rates sit far below aliasing
//! frequencies). `Random` is a sample-and-hold updated once per cycle.

use crate::math::lookup_sine;
use crate::noise::xorshift64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LfoShape {
    Sine,
    Triangle,
    SawUp,
    SawDown,
    Square,
    Random,
}

impl LfoShape {
    pub const ALL: [LfoShape; 6] = [
        LfoShape::Sine,
        LfoShape::Triangle,
        LfoShape::SawUp,
        LfoShape::SawDown,
        LfoShape::Square,
        LfoShape::Random,
    ];

    pub fn label(self) -> &'static str {
        match self {
            LfoShape::Sine => "Sine",
            LfoShape::Triangle => "Tri",
            LfoShape::SawUp => "Saw+",
            LfoShape::SawDown => "Saw-",
            LfoShape::Square => "Square",
            LfoShape::Random => "S&H",
        }
    }

    /// Phase in `[0, 1)` at which this shape's output is zero — the clean
    /// restart point so a (re)triggered LFO eases out of zero rather than
    /// stepping to an extreme. Square and Random have no zero crossing, so they
    /// restart at the cycle boundary (phase 0).
    #[inline]
    pub fn zero_crossing_phase(self) -> f32 {
        match self {
            LfoShape::Sine => 0.0,      // sin(0) = 0, rising
            LfoShape::Triangle => 0.25, // -1 at 0, 0 at 0.25 rising, +1 at 0.5
            LfoShape::SawUp => 0.5,     // 2p-1 = 0
            LfoShape::SawDown => 0.5,   // 1-2p = 0
            LfoShape::Square => 0.0,    // no zero crossing — start of the cycle
            LfoShape::Random => 0.0,    // stepped — start of the cycle
        }
    }
}

#[derive(Clone)]
pub struct LfoCore {
    sample_rate: f32,
    phase: f32,
    phase_increment: f32,
    prng_state: u64,
    random_value: f32,
}

impl LfoCore {
    pub fn new(sample_rate: f32, seed: u64) -> Self {
        Self {
            sample_rate,
            phase: 0.0,
            phase_increment: 1.0 / sample_rate,
            prng_state: seed | 1,
            random_value: 0.0,
        }
    }

    /// Reset phase to the cycle boundary (phase 0). Used for a full engine reset
    /// where no note context applies.
    pub fn reset(&mut self) {
        self.phase = 0.0;
    }

    /// Retrigger to `shape`'s zero crossing (E005 / 0018) so a per-voice note-on
    /// starts modulation from zero rather than stepping to an extreme. Square /
    /// S&H have no zero crossing and restart at the cycle boundary.
    #[inline]
    pub fn retrigger(&mut self, shape: LfoShape) {
        self.phase = shape.zero_crossing_phase();
    }

    /// Current phase in `[0, 1)`. Exposed so callers (and tests) can observe a
    /// note-on retrigger.
    #[inline]
    pub fn phase(&self) -> f32 {
        self.phase
    }

    /// Set rate in Hz (clamped to a musical LFO range).
    #[inline]
    pub fn set_rate(&mut self, hz: f32) {
        self.phase_increment = hz.clamp(0.001, 40.0) / self.sample_rate;
    }

    /// Advance one sample and return the bipolar `[-1, 1]` value for `shape`.
    #[inline]
    pub fn next(&mut self, shape: LfoShape) -> f32 {
        let next = self.phase + self.phase_increment;
        if next >= 1.0 {
            self.phase = next - 1.0;
            self.random_value = xorshift64(&mut self.prng_state);
        } else {
            self.phase = next;
        }
        let p = self.phase;
        match shape {
            LfoShape::Sine => lookup_sine(p),
            LfoShape::Triangle => 1.0 - 4.0 * (p - 0.5).abs(),
            LfoShape::SawUp => 2.0 * p - 1.0,
            LfoShape::SawDown => 1.0 - 2.0 * p,
            LfoShape::Square => {
                if p < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            LfoShape::Random => self.random_value,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn period_matches_rate() {
        let sr = 1000.0;
        let mut lfo = LfoCore::new(sr, 1);
        lfo.set_rate(10.0); // 100-sample period
        // Find first up-zero-crossing period of the sine.
        let mut prev = lfo.next(LfoShape::Sine);
        let mut crossings = vec![];
        for i in 1..1000 {
            let v = lfo.next(LfoShape::Sine);
            if prev < 0.0 && v >= 0.0 {
                crossings.push(i);
            }
            prev = v;
        }
        assert!(crossings.len() >= 2);
        let period = (crossings[1] - crossings[0]) as i64;
        assert!((period - 100).abs() <= 2, "period {period}");
    }

    #[test]
    fn retrigger_starts_near_zero_for_crossing_shapes() {
        // Sine / Triangle / Saws have a zero crossing: retriggering then taking
        // the next sample yields ~0 (within one slow-rate phase increment), so
        // modulation eases out of zero. Square / S&H restart at the boundary.
        let sr = 48_000.0;
        for shape in [
            LfoShape::Sine,
            LfoShape::Triangle,
            LfoShape::SawUp,
            LfoShape::SawDown,
        ] {
            let mut lfo = LfoCore::new(sr, 7);
            lfo.set_rate(2.0); // slow → tiny increment per sample
            for _ in 0..1234 {
                lfo.next(shape);
            }
            lfo.retrigger(shape);
            assert_eq!(lfo.phase(), shape.zero_crossing_phase());
            let v = lfo.next(shape);
            assert!(v.abs() < 0.01, "{shape:?} did not restart near zero: {v}");
        }
    }

    #[test]
    fn all_shapes_bipolar_bounded() {
        let mut lfo = LfoCore::new(48000.0, 3);
        lfo.set_rate(5.0);
        for shape in LfoShape::ALL {
            for _ in 0..20_000 {
                let v = lfo.next(shape);
                assert!(v.is_finite() && v.abs() <= 1.001, "{shape:?} {v}");
            }
        }
    }
}
