//! Noise sources for the mixer. White (flat) and Pink (−3 dB/oct); Brown was
//! dropped in the E006 fixed-panel pass (a 2-button White/Pink selector).
//!
//! [`PolyNoise`] is the structure-of-arrays sibling used by the voice bank: one
//! independent PRNG + pink shaper per channel, so a stacked (unison) note sums
//! decorrelated noise streams rather than one comb-coherent copy.

use crate::math::xorshift64;

/// Selectable noise colour for the mixer's noise source.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum NoiseColor {
    #[default]
    White,
    Pink,
}

impl NoiseColor {
    pub const ALL: [NoiseColor; 2] = [NoiseColor::White, NoiseColor::Pink];

    pub fn label(self) -> &'static str {
        match self {
            NoiseColor::White => "White",
            NoiseColor::Pink => "Pink",
        }
    }
}

/// 3-pole IIR pink-shaping filter (Paul Kellett / Voss–McCartney). −3 dB/oct
/// across the audio band from flat white input.
#[derive(Clone, Copy)]
struct PinkFilter {
    b0: f32,
    b1: f32,
    b2: f32,
}

impl PinkFilter {
    #[inline]
    fn new() -> Self {
        Self {
            b0: 0.0,
            b1: 0.0,
            b2: 0.0,
        }
    }

    #[inline]
    fn process(&mut self, white: f32) -> f32 {
        self.b0 = 0.99765 * self.b0 + white * 0.0990460;
        self.b1 = 0.96300 * self.b1 + white * 0.2965164;
        self.b2 = 0.57000 * self.b2 + white * 1.0526913;
        (self.b0 + self.b1 + self.b2 + white * 0.1848) * 0.11
    }
}

/// Decorrelated per-channel noise seed from the layer's base seed (distinct
/// stream from the LFO/unison seeds), forced non-zero (xorshift64 sticks at 0).
#[inline]
fn noise_seed(base: u64, ch: usize) -> u64 {
    base.wrapping_mul(0x2545_F491_4F6C_DD1D)
        .wrapping_add((ch as u64 + 1).wrapping_mul(0x9E37_79B1))
        | 1
}

/// 16-voice (per-layer `N`) noise generator in structure-of-arrays form: one
/// PRNG state and pink shaper per channel.
#[derive(Clone)]
pub struct PolyNoise<const N: usize> {
    state: [u64; N],
    pink: [PinkFilter; N],
}

impl<const N: usize> PolyNoise<N> {
    pub fn new(seed: u64) -> Self {
        Self {
            state: std::array::from_fn(|ch| noise_seed(seed, ch)),
            pink: [PinkFilter::new(); N],
        }
    }

    pub fn reset(&mut self) {
        self.pink = [PinkFilter::new(); N];
    }

    /// One sample per channel into `out`, shaped by `color` (a layer-wide
    /// selector, so the colour branch hoists outside the lane loop).
    #[inline]
    pub fn process(&mut self, color: NoiseColor, out: &mut [f32; N]) {
        match color {
            NoiseColor::White => {
                for (o, s) in out.iter_mut().zip(self.state.iter_mut()) {
                    *o = xorshift64(s);
                }
            }
            NoiseColor::Pink => {
                for ((o, s), p) in out
                    .iter_mut()
                    .zip(self.state.iter_mut())
                    .zip(self.pink.iter_mut())
                {
                    *o = p.process(xorshift64(s));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rms(s: &[f32]) -> f32 {
        (s.iter().map(|x| x * x).sum::<f32>() / s.len() as f32).sqrt()
    }

    #[test]
    fn white_is_bounded_and_nonzero() {
        let mut ns = PolyNoise::<4>::new(7);
        let mut acc = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
        let mut out = [0.0f32; 4];
        for _ in 0..20_000 {
            ns.process(NoiseColor::White, &mut out);
            for (a, &o) in acc.iter_mut().zip(out.iter()) {
                assert!(o.is_finite() && o.abs() <= 1.001, "{o}");
                a.push(o);
            }
        }
        for (v, a) in acc.iter().enumerate() {
            assert!(rms(a) > 0.3, "white channel {v} near-silent");
        }
    }

    #[test]
    fn channels_are_decorrelated() {
        // Independent per-channel PRNGs: two channels' white streams must not be
        // identical (a shared seed would comb to a coherent copy under unison).
        let mut ns = PolyNoise::<2>::new(42);
        let mut same = 0;
        let mut out = [0.0f32; 2];
        for _ in 0..10_000 {
            ns.process(NoiseColor::White, &mut out);
            if out[0] == out[1] {
                same += 1;
            }
        }
        assert!(
            same < 10,
            "channels too correlated: {same} identical samples"
        );
    }

    #[test]
    fn pink_has_more_low_freq_energy_than_white() {
        // Pink is low-pass-tilted relative to white: the lag-1 autocorrelation of
        // pink is markedly positive (neighbouring samples track), white's ~0.
        let mut ns = PolyNoise::<1>::new(3);
        let collect = |ns: &mut PolyNoise<1>, c| {
            let mut out = [0.0f32; 1];
            let mut xs = Vec::with_capacity(40_000);
            for _ in 0..40_000 {
                ns.process(c, &mut out);
                xs.push(out[0]);
            }
            xs
        };
        let lag1 = |xs: &[f32]| {
            let mean = xs.iter().sum::<f32>() / xs.len() as f32;
            let var = xs.iter().map(|x| (x - mean).powi(2)).sum::<f32>();
            let cov: f32 = xs.windows(2).map(|w| (w[0] - mean) * (w[1] - mean)).sum();
            cov / var
        };
        let white = collect(&mut ns, NoiseColor::White);
        ns.reset();
        let pink = collect(&mut ns, NoiseColor::Pink);
        assert!(lag1(&white).abs() < 0.1, "white lag1 {}", lag1(&white));
        assert!(lag1(&pink) > 0.3, "pink lag1 {}", lag1(&pink));
    }

    #[test]
    fn pink_bounded() {
        let mut ns = PolyNoise::<2>::new(11);
        let mut out = [0.0f32; 2];
        for _ in 0..50_000 {
            ns.process(NoiseColor::Pink, &mut out);
            for &o in out.iter() {
                assert!(o.is_finite() && o.abs() <= 1.5, "{o}");
            }
        }
    }
}
