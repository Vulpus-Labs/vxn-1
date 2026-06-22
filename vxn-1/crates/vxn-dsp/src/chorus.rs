//! Stereo chorus: the `VChorus` vintage bucket-brigade-device (BBD) emulation,
//! ported from `patches-bundles::patches-vintage`, voiced after an early-80s
//! Roland BBD chorus ("bright" / Juno-60 reference).
//!
//! Two [`ModDelayLine`]s (the BBD's input anti-image bank → soft bucket
//! saturation → fractional read → output reconstruction bank → variant trim)
//! are swept by a single strict-triangle LFO. The right channel reads the
//! *inverted* LFO — the authentic mono-compatible stereo trick, not two
//! phase-offset LFOs. Broadband BBD hiss and clock jitter are modelled but
//! default to silent/off.
//!
//! ## Block processing
//!
//! The engine drives this once per [`CONTROL_BLOCK`]. All control-rate
//! quantities — LFO increment, delay centre/swing, dry/wet gains, hiss floor —
//! are hoisted out of the inner loop by [`set_params`](StereoChorus::set_params)
//! and [`process_block`](StereoChorus::process_block) (the old per-sample
//! `process` recomputed the LFO increment, a divide, every sample). The block
//! method also runs each delay line as its own pass so its filter-bank and ring
//! state stay hot in cache for the whole block.

use crate::CONTROL_BLOCK;
use crate::delay_line::{Interp, ModDelayLine};
use crate::math::xorshift64;

/// Bright (Juno-60 reference) delay sweep, in seconds: 1.66–5.35 ms.
const DELAY_MIN_S: f32 = 0.00166;
const DELAY_MAX_S: f32 = 0.00535;
/// Ring headroom — the largest delay any setting commands, with margin.
const MAX_DELAY_S: f32 = 0.008;
/// Write soft-saturation drive, matching `BbdDevice::BBD_256`.
const SAT_DRIVE: f32 = 1.2;
/// Post-BBD reconstruction trim for the bright voicing.
const RECON_CUTOFF_HZ: f32 = 9_000.0;
/// Bright summing runs the wet a touch hotter than the dry (≈ 1:1.15).
const WET_GAIN: f32 = 1.15;
/// Broadband uncompanded hiss floor at `hiss = 1.0` (bright is ~-54 dBFS).
const HISS_FLOOR: f32 = 0.0020;

#[inline]
fn center_s() -> f32 {
    0.5 * (DELAY_MIN_S + DELAY_MAX_S)
}
#[inline]
fn swing_s() -> f32 {
    0.5 * (DELAY_MAX_S - DELAY_MIN_S)
}

/// Strict triangle LFO in `[-1, +1]`, phase wrapped to `[0, 1)`.
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

    #[inline]
    fn tick(&mut self) -> f32 {
        self.phase += self.increment;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        let p = self.phase;
        (4.0 * (p - (p + 0.5).floor()).abs() - 1.0).clamp(-1.0, 1.0)
    }
}

/// Stereo BBD chorus (`VChorus`, bright voicing). The engine keeps its existing
/// three controls: `rate_hz` drives the LFO, `depth` scales the delay swing,
/// `mix` is the dry/wet blend. Variant (bright/dark), mode, hiss and jitter are
/// further `VChorus` knobs not yet surfaced as plugin params; bright + full
/// modulation + silent hiss + no jitter is the default voicing.
#[derive(Clone)]
pub struct StereoChorus {
    sample_rate: f32,
    left: ModDelayLine,
    right: ModDelayLine,
    lfo: TriangleLfo,
    noise_state: u64,
    // Control-block parameters.
    depth: f32, // 0..1 → fraction of the swing actually used
    mix: f32,
    hiss_amount: f32,
}

impl StereoChorus {
    pub fn new(sample_rate: f32) -> Self {
        let mut left = ModDelayLine::new(MAX_DELAY_S, sample_rate);
        let mut right = ModDelayLine::new(MAX_DELAY_S, sample_rate);
        for line in [&mut left, &mut right] {
            line.set_saturation(SAT_DRIVE);
            line.set_recon_cutoff(RECON_CUTOFF_HZ);
            // Thiran read: flat magnitude + group delay tracks the BBD's clean
            // analog delay best under the smooth Juno-style sweep.
            line.set_interp(Interp::Thiran);
        }
        // Decorrelate the (currently disabled) jitter walks across channels.
        left.set_jitter_seed(0x1BBD_0001);
        right.set_jitter_seed(0x1BBD_0002);
        Self {
            sample_rate,
            left,
            right,
            lfo: TriangleLfo::new(),
            noise_state: 0x5DE5,
            depth: 0.5,
            mix: 0.5,
            hiss_amount: 0.0,
        }
    }

    pub fn clear(&mut self) {
        self.left.clear();
        self.right.clear();
        self.lfo.phase = 0.0;
    }

    /// Set parameters for the next control block. `rate_hz` typically 0.1–6 Hz,
    /// `depth` and `mix` in `[0, 1]`. The LFO increment is computed here, once
    /// per block, rather than per sample.
    pub fn set_params(&mut self, rate_hz: f32, depth: f32, mix: f32) {
        self.lfo
            .set_rate(rate_hz.clamp(0.01, 12.0), self.sample_rate);
        self.depth = depth.clamp(0.0, 1.0);
        self.mix = mix.clamp(0.0, 1.0);
    }

    /// Broadband BBD hiss amount in `[0, 1]`. `0.0` (the default) keeps the
    /// effect silent when idle; `1.0` is the faithful uncompanded floor.
    pub fn set_hiss(&mut self, amount: f32) {
        self.hiss_amount = amount.clamp(0.0, 1.0);
    }

    /// Clock-jitter amount in `[0, 1]` (delay-line clock drift). `0.0` disables.
    pub fn set_jitter(&mut self, amount: f32) {
        self.left.set_jitter_amount(amount);
        self.right.set_jitter_amount(amount);
    }

    /// Stereo-in block variant for Stereo routing mode: the FX bus carries true
    /// L/R rather than a mono sum, so each delay line takes its own channel as
    /// input. The single LFO (inverted for R) and wet/dry law are unchanged.
    pub fn process_block_stereo(
        &mut self,
        l_in: &[f32],
        r_in: &[f32],
        out_l: &mut [f32],
        out_r: &mut [f32],
    ) {
        let n = l_in
            .len()
            .min(r_in.len())
            .min(out_l.len())
            .min(out_r.len());

        let center = center_s();
        let swing = swing_s() * self.depth;
        let min_d = (center - swing_s()).max(1.0e-4);
        let max_d = center + swing_s();
        // Equal-power crossfade (wet decorrelated by modulated delay); WET_GAIN
        // keeps the intentional bright tilt over the sqrt wet leg.
        let mix = self.mix;
        let dry_gain = (1.0 - mix).sqrt();
        let wet_gain = WET_GAIN * mix.sqrt();
        let floor = HISS_FLOOR * self.hiss_amount;

        let mut dl = [0.0f32; CONTROL_BLOCK];
        let mut dr = [0.0f32; CONTROL_BLOCK];
        let mut nl = [0.0f32; CONTROL_BLOCK];
        let mut nr = [0.0f32; CONTROL_BLOCK];
        for i in 0..n {
            let lfo = self.lfo.tick();
            dl[i] = (center + swing * lfo).clamp(min_d, max_d);
            dr[i] = (center - swing * lfo).clamp(min_d, max_d);
            nl[i] = xorshift64(&mut self.noise_state) * floor;
            nr[i] = xorshift64(&mut self.noise_state) * floor;
            out_l[i] = l_in[i] * dry_gain;
            out_r[i] = r_in[i] * dry_gain;
        }

        for i in 0..n {
            let wet = self.left.process(l_in[i] + nl[i], dl[i]);
            out_l[i] += wet_gain * wet;
        }
        for i in 0..n {
            let wet = self.right.process(r_in[i] + nr[i], dr[i]);
            out_r[i] += wet_gain * wet;
        }
    }

    /// Process one stereo sample. Convenience wrapper over the BBD core for
    /// callers outside the block engine; the mono sum drives both lines.
    #[inline]
    pub fn process(&mut self, in_l: f32, in_r: f32) -> (f32, f32) {
        let mono = 0.5 * (in_l + in_r);
        let center = center_s();
        let swing = swing_s() * self.depth;
        let min_d = (center - swing_s()).max(1.0e-4);
        let max_d = center + swing_s();
        let floor = HISS_FLOOR * self.hiss_amount;

        let lfo = self.lfo.tick();
        let dl = (center + swing * lfo).clamp(min_d, max_d);
        let dr = (center - swing * lfo).clamp(min_d, max_d);
        let nl = xorshift64(&mut self.noise_state) * floor;
        let nr = xorshift64(&mut self.noise_state) * floor;

        let wet_l = self.left.process(mono + nl, dl);
        let wet_r = self.right.process(mono + nr, dr);

        // Equal-power crossfade; WET_GAIN keeps the bright tilt (see block path).
        let m = self.mix;
        let dry = (1.0 - m).sqrt();
        let wet = m.sqrt();
        (
            in_l * dry + WET_GAIN * wet_l * wet,
            in_r * dry + WET_GAIN * wet_r * wet,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::lookup_sine;

    #[test]
    fn output_finite_and_passes_signal() {
        // The BBD banks rely on the audio thread's flush-to-zero rather than
        // per-lane denormal flushing; mirror that contract in the test.
        crate::enable_flush_to_zero();
        let sr = 48_000.0;
        let mut c = StereoChorus::new(sr);
        c.set_params(1.0, 0.7, 0.5);
        let mut energy = 0.0f32;
        for i in 0..48_000 {
            let x = lookup_sine((i as f32 * 220.0 / sr).fract());
            let (l, r) = c.process(x, x);
            assert!(l.is_finite() && r.is_finite());
            energy += l.abs();
        }
        assert!(energy > 100.0, "chorus produced near-silence");
    }

    #[test]
    fn block_matches_per_sample() {
        // The block path and the per-sample wrapper share the same core; with a
        // mono source (l == r) they must agree sample-for-sample.
        crate::enable_flush_to_zero();
        let sr = 48_000.0;
        let mut a = StereoChorus::new(sr);
        let mut b = StereoChorus::new(sr);
        a.set_params(0.6, 0.5, 0.4);
        b.set_params(0.6, 0.5, 0.4);

        let mut dry = [0.0f32; CONTROL_BLOCK];
        let mut bl = [0.0f32; CONTROL_BLOCK];
        let mut br = [0.0f32; CONTROL_BLOCK];
        for blk in 0..32 {
            for (i, d) in dry.iter_mut().enumerate() {
                let phase = ((blk * CONTROL_BLOCK + i) as f32 * 330.0 / sr).fract();
                *d = lookup_sine(phase);
            }
            b.process_block_stereo(&dry, &dry, &mut bl, &mut br);
            for (i, &d) in dry.iter().enumerate() {
                let (l, r) = a.process(d, d);
                assert!(
                    (l - bl[i]).abs() < 1e-5,
                    "L mismatch blk{blk} i{i}: {l} vs {}",
                    bl[i]
                );
                assert!(
                    (r - br[i]).abs() < 1e-5,
                    "R mismatch blk{blk} i{i}: {r} vs {}",
                    br[i]
                );
            }
        }
    }

    #[test]
    fn stereo_in_processes_channels_independently() {
        // Sine on L, silence on R: L output must carry the signal, R must be
        // essentially silent. The R line still ticks the noise/LFO machinery
        // but with zero input + zero hiss it contributes nothing audible.
        crate::enable_flush_to_zero();
        let sr = 48_000.0;
        let mut c = StereoChorus::new(sr);
        c.set_params(1.0, 0.7, 0.5);

        let mut l_in = [0.0f32; CONTROL_BLOCK];
        let r_in = [0.0f32; CONTROL_BLOCK];
        let mut l_out = [0.0f32; CONTROL_BLOCK];
        let mut r_out = [0.0f32; CONTROL_BLOCK];
        let mut l_energy = 0.0f32;
        let mut r_energy = 0.0f32;
        let blocks = 48_000 / CONTROL_BLOCK;
        for blk in 0..blocks {
            for (i, d) in l_in.iter_mut().enumerate() {
                let phase = ((blk * CONTROL_BLOCK + i) as f32 * 220.0 / sr).fract();
                *d = lookup_sine(phase);
            }
            c.process_block_stereo(&l_in, &r_in, &mut l_out, &mut r_out);
            for i in 0..CONTROL_BLOCK {
                assert!(l_out[i].is_finite() && r_out[i].is_finite());
                l_energy += l_out[i].abs();
                r_energy += r_out[i].abs();
            }
        }
        assert!(l_energy > 100.0, "L should carry the sine plus wet");
        assert!(
            r_energy < 1.0e-3,
            "R should be silent with zero input and zero hiss; got {r_energy}"
        );
    }

    #[test]
    fn hiss_floor_is_audible_when_enabled() {
        crate::enable_flush_to_zero();
        let sr = 48_000.0;
        let mut c = StereoChorus::new(sr);
        c.set_params(0.5, 0.5, 1.0);
        c.set_hiss(1.0);
        let mut energy = 0.0f32;
        for _ in 0..48_000 {
            let (l, _) = c.process(0.0, 0.0); // silent input
            energy += l.abs();
        }
        assert!(energy > 0.0, "hiss should leak through on silence");
    }
}
