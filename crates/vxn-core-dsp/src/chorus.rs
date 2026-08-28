//! Stereo chorus: a vintage bucket-brigade-device (BBD) emulation.
//!
//! Two [`ModDelayLine`]s (the BBD's input anti-image bank → soft bucket
//! saturation → fractional read → output reconstruction bank → variant trim)
//! are swept by a single strict-triangle LFO. The right channel reads the
//! *inverted* LFO — the authentic mono-compatible stereo trick, not two
//! phase-offset LFOs. Broadband BBD hiss and clock jitter are modelled but
//! default to silent/off.
//!
//! ## True stereo only
//!
//! There used to be two non-equivalent entry points: a block path taking real
//! L/R, and a per-sample `process` that **mono-summed** its input before
//! feeding both lines. vxn-1's engine used the first, vxn-1b's `FxChain` the
//! second, so the same kernel made two different sounds depending on who called
//! it. Ticket 0229 keeps the true-stereo behaviour and deletes the mono sum:
//! [`FxKernel::process`] now feeds each line its own channel.
//!
//! ## Block processing
//!
//! All control-rate quantities — LFO increment, delay centre/swing, hiss floor
//! — are hoisted out of the inner loop by [`FxKernel::set_params`].
//! [`FxKernel::process_block`] additionally runs each delay line as its own
//! pass so its filter-bank and ring state stay hot in cache for the whole
//! block, and is sample-identical to looping [`FxKernel::process`].

use vxn_core_utils::math::xorshift64;

use crate::control::CONTROL_BLOCK;
use crate::declick::WetFade;
use crate::delay_line::{Interp, ModDelayLine};
use crate::fx::FxKernel;

/// Bright delay sweep, in seconds: 1.66–5.35 ms.
const DELAY_MIN_S: f32 = 0.00166;
const DELAY_MAX_S: f32 = 0.00535;
/// Ring headroom — the largest delay any setting commands, with margin.
const MAX_DELAY_S: f32 = 0.008;
/// Write soft-saturation drive.
const SAT_DRIVE: f32 = 1.2;
/// Post-BBD reconstruction trim for the bright voicing.
const RECON_CUTOFF_HZ: f32 = 9_000.0;
/// Bright summing runs the wet a touch hotter than the dry (≈ 1:1.15).
const WET_GAIN: f32 = 1.15;
/// Broadband uncompanded hiss floor at `hiss = 1.0` (bright is ~-54 dBFS).
const HISS_FLOOR: f32 = 0.0020;
/// Dry/wet glide time, matching the phaser's. Long enough to mask a knob jump
/// or a switch-on fade-in, short enough to feel instant.
const MIX_SMOOTH_MS: f32 = 30.0;


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

/// Block-rate parameter snapshot the engine fans into [`StereoChorus`].
///
/// `hiss` and `jitter` were `set_hiss` / `set_jitter` methods that no shipping
/// synth ever called — modelled BBD character with no control surface. They are
/// here rather than deleted because the model is real and a synth may want it;
/// [`Default`] keeps the shipped voicing (both off).
#[derive(Clone, Copy, Debug)]
pub struct ChorusParams {
    pub on: bool,
    /// LFO rate, Hz (clamped to 0.01..12 by `set_params`).
    pub rate_hz: f32,
    /// Fraction of the delay swing actually used, 0..1.
    pub depth: f32,
    /// Dry/wet, 0..1. Blended equal-power.
    pub mix: f32,
    /// Broadband BBD hiss, 0..1. `0.0` keeps the effect silent when idle;
    /// `1.0` is the faithful uncompanded floor.
    pub hiss: f32,
    /// Delay-line clock-jitter amount, 0..1. `0.0` disables.
    pub jitter: f32,
}

impl Default for ChorusParams {
    fn default() -> Self {
        Self { on: false, rate_hz: 1.0, depth: 0.5, mix: 0.5, hiss: 0.0, jitter: 0.0 }
    }
}

/// Stereo BBD chorus (bright voicing). `rate_hz` drives the LFO, `depth` scales
/// the delay swing, `mix` is the dry/wet blend. Bright + full modulation +
/// silent hiss + no jitter is the default voicing.
///
/// Bypass is internal, via [`WetFade`] — do not wrap this in an outer crossfade
/// as well (E041's double-fade ban). Gate on [`FxKernel::is_active`] and skip.
#[derive(Clone)]
pub struct StereoChorus {
    sample_rate: f32,
    left: ModDelayLine,
    right: ModDelayLine,
    lfo: TriangleLfo,
    noise_state: u64,
    // Control-block parameters.
    depth: f32, // 0..1 → fraction of the swing actually used
    hiss_amount: f32,
    /// Enable gate and smoothed dry/wet in one.
    fade: WetFade,
}

/// The per-sample control quantities the block path hoists into scratch.
struct Tick {
    delay_l: f32,
    delay_r: f32,
    noise_l: f32,
    noise_r: f32,
    wet_gain: f32,
}

impl StereoChorus {
    /// Advance the LFO, noise and fade by one sample and return what the wet
    /// path needs. The single place that ordering is defined, so the per-sample
    /// and block paths cannot drift apart.
    #[inline]
    fn tick(&mut self) -> Tick {
        let center = center_s();
        let swing = swing_s() * self.depth;
        let min_d = (center - swing_s()).max(1.0e-4);
        let max_d = center + swing_s();
        let floor = HISS_FLOOR * self.hiss_amount;

        let lfo = self.lfo.tick();
        // The right line reads the *inverted* LFO — the authentic
        // mono-compatible stereo trick, not two phase-offset LFOs.
        let delay_l = (center + swing * lfo).clamp(min_d, max_d);
        let delay_r = (center - swing * lfo).clamp(min_d, max_d);
        let noise_l = xorshift64(&mut self.noise_state) * floor;
        let noise_r = xorshift64(&mut self.noise_state) * floor;
        let (mix, _edge) = self.fade.tick();
        Tick { delay_l, delay_r, noise_l, noise_r, wet_gain: mix }
    }

    /// Equal-power dry/wet; `WET_GAIN` keeps the intentional bright tilt over
    /// the `sqrt` wet leg.
    #[inline]
    fn gains(mix: f32) -> (f32, f32) {
        ((1.0 - mix).sqrt(), WET_GAIN * mix.sqrt())
    }

    /// Block variant, out of place. Same body as [`FxKernel::process_block`];
    /// kept because the two have different aliasing rules and the
    /// channel-independence test needs the out-of-place form.
    pub fn process_block_stereo(
        &mut self,
        l_in: &[f32],
        r_in: &[f32],
        out_l: &mut [f32],
        out_r: &mut [f32],
    ) {
        let n = l_in.len().min(r_in.len()).min(out_l.len()).min(out_r.len());
        out_l[..n].copy_from_slice(&l_in[..n]);
        out_r[..n].copy_from_slice(&r_in[..n]);
        self.process_block(&mut out_l[..n], &mut out_r[..n]);
    }
}

impl FxKernel for StereoChorus {
    type Params = ChorusParams;

    fn new(sample_rate: f32) -> Self {
        let mut left = ModDelayLine::new(MAX_DELAY_S, sample_rate);
        let mut right = ModDelayLine::new(MAX_DELAY_S, sample_rate);
        for line in [&mut left, &mut right] {
            line.set_saturation(SAT_DRIVE);
            line.set_recon_cutoff(RECON_CUTOFF_HZ);
            // Thiran read: flat magnitude + group delay tracks the BBD's clean
            // analog delay best under the smooth sweep.
            line.set_interp(Interp::Thiran);
        }
        // Decorrelate the (default-disabled) jitter walks across channels.
        left.set_jitter_seed(0x1BBD_0001);
        right.set_jitter_seed(0x1BBD_0002);
        Self {
            sample_rate,
            left,
            right,
            lfo: TriangleLfo::new(),
            noise_state: 0x5DE5,
            depth: 0.5,
            hiss_amount: 0.0,
            fade: WetFade::new(MIX_SMOOTH_MS, sample_rate),
        }
    }

    /// The LFO increment is computed here, once per block, rather than per
    /// sample. `on` and `mix` go through [`WetFade::set`] as a pair so the first
    /// snapshot snaps rather than gliding in.
    fn set_params(&mut self, p: &ChorusParams) {
        self.lfo.set_rate(p.rate_hz.clamp(0.01, 12.0), self.sample_rate);
        self.depth = p.depth.clamp(0.0, 1.0);
        self.hiss_amount = p.hiss.clamp(0.0, 1.0);
        let jitter = p.jitter.clamp(0.0, 1.0);
        self.left.set_jitter_amount(jitter);
        self.right.set_jitter_amount(jitter);
        self.fade.set(p.on, p.mix);
    }

    /// One stereo sample in / out. Each delay line takes **its own channel** —
    /// the pre-0229 mono sum is gone.
    #[inline]
    fn process(&mut self, in_l: f32, in_r: f32) -> (f32, f32) {
        if self.fade.settled_off() {
            return (in_l, in_r);
        }
        let t = self.tick();
        let wet_l = self.left.process(in_l + t.noise_l, t.delay_l);
        let wet_r = self.right.process(in_r + t.noise_r, t.delay_r);
        let (dry_g, wet_g) = Self::gains(t.wet_gain);
        (in_l * dry_g + wet_g * wet_l, in_r * dry_g + wet_g * wet_r)
    }

    /// Runs each delay line as its own pass, so its filter-bank and ring state
    /// stay hot in cache for the whole block. Sample-identical to looping
    /// [`process`](FxKernel::process) — `assert_block_matches_sample` is the
    /// check.
    fn process_block(&mut self, l: &mut [f32], r: &mut [f32]) {
        debug_assert_eq!(l.len(), r.len(), "stereo block halves must match");
        // Chunked because the scratch is fixed-size. The pre-0229 body indexed
        // `[f32; CONTROL_BLOCK]` with the caller's length and would have
        // panicked on a longer block; nothing called it that way, but
        // `FxKernel::process_block` takes arbitrary slices.
        for (lc, rc) in l.chunks_mut(CONTROL_BLOCK).zip(r.chunks_mut(CONTROL_BLOCK)) {
            self.process_chunk(lc, rc);
        }
    }

    fn reset(&mut self) {
        self.clear();
        self.fade.reset();
    }

    fn clear(&mut self) {
        self.left.clear();
        self.right.clear();
        self.lfo.phase = 0.0;
    }

    #[inline]
    fn is_active(&self) -> bool {
        self.fade.is_active()
    }
}

impl StereoChorus {
    /// One `CONTROL_BLOCK`-or-shorter chunk of [`FxKernel::process_block`].
    fn process_chunk(&mut self, l: &mut [f32], r: &mut [f32]) {
        let n = l.len().min(r.len());
        let mut ticks: [Tick; CONTROL_BLOCK] = std::array::from_fn(|_| Tick {
            delay_l: 0.0,
            delay_r: 0.0,
            noise_l: 0.0,
            noise_r: 0.0,
            wet_gain: 0.0,
        });
        let mut dry = [(0.0f32, 0.0f32); CONTROL_BLOCK];

        // A switch-off fade can land partway through a block. Past that point
        // the per-sample path returns its input untouched and stops ticking, so
        // the block path has to stop at the same sample or the two diverge.
        let mut active = 0;
        while active < n {
            if self.fade.settled_off() {
                break;
            }
            ticks[active] = self.tick();
            dry[active] = (l[active], r[active]);
            active += 1;
        }

        for i in 0..active {
            let wet = self.left.process(dry[i].0 + ticks[i].noise_l, ticks[i].delay_l);
            let (dry_g, wet_g) = Self::gains(ticks[i].wet_gain);
            l[i] = dry[i].0 * dry_g + wet_g * wet;
        }
        for i in 0..active {
            let wet = self.right.process(dry[i].1 + ticks[i].noise_r, ticks[i].delay_r);
            let (dry_g, wet_g) = Self::gains(ticks[i].wet_gain);
            r[i] = dry[i].1 * dry_g + wet_g * wet;
        }
        // Settled off from `active` on — bit-exact passthrough, already in place.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util;
    use std::f32::consts::TAU;

    /// Cheap deterministic sine over a `[0, 1)` phase — the shared crate has no
    /// oscillator, and these tests only need a periodic source.
    fn sine_01(phase: f32) -> f32 {
        (phase * TAU).sin()
    }

    fn on(rate_hz: f32, depth: f32, mix: f32) -> ChorusParams {
        ChorusParams { on: true, rate_hz, depth, mix, ..ChorusParams::default() }
    }

    #[test]
    fn output_finite_and_passes_signal() {
        // The BBD banks rely on the audio thread's flush-to-zero rather than
        // per-lane denormal flushing; mirror that contract in the test.
        let _ftz = vxn_core_utils::ScopedFlushToZero::new();
        let sr = 48_000.0;
        let mut c = StereoChorus::new(sr);
        c.set_params(&on(1.0, 0.7, 0.5));
        let mut energy = 0.0f32;
        for i in 0..48_000 {
            let x = sine_01((i as f32 * 220.0 / sr).fract());
            let (l, r) = c.process(x, x);
            assert!(l.is_finite() && r.is_finite());
            energy += l.abs();
        }
        assert!(energy > 100.0, "chorus produced near-silence");
    }

    #[test]
    fn block_matches_per_sample() {
        // Before 0229 this could only be asserted for a MONO source, because
        // per-sample `process` summed L and R. Both paths are true stereo now,
        // so a genuinely stereo source must agree too — which is what
        // `assert_block_matches_sample` drives.
        let _ftz = vxn_core_utils::ScopedFlushToZero::new();
        let sr = 48_000.0;
        let mut a = StereoChorus::new(sr);
        let mut b = StereoChorus::new(sr);
        a.set_params(&on(0.6, 0.5, 0.4));
        b.set_params(&on(0.6, 0.5, 0.4));

        let mut dry = [0.0f32; CONTROL_BLOCK];
        let mut bl = [0.0f32; CONTROL_BLOCK];
        let mut br = [0.0f32; CONTROL_BLOCK];
        for blk in 0..32 {
            for (i, d) in dry.iter_mut().enumerate() {
                let phase = ((blk * CONTROL_BLOCK + i) as f32 * 330.0 / sr).fract();
                *d = sine_01(phase);
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

    /// The `FxKernel` contract: the block override must be sample-identical to
    /// looping `process`, on a stereo source.
    #[test]
    fn block_override_matches_the_sample_path() {
        let _ftz = vxn_core_utils::ScopedFlushToZero::new();
        test_util::assert_block_matches_sample(
            || StereoChorus::new(48_000.0),
            &on(0.6, 0.5, 0.4),
            96,
        );
    }

    /// A switch-off fade can land partway through a block, after which the
    /// per-sample path passes its input through untouched and stops ticking.
    /// The block path has to stop at the same sample or the two diverge — this
    /// is the case the chunked `active` prefix exists for.
    #[test]
    fn block_and_sample_agree_across_a_switch_off() {
        let _ftz = vxn_core_utils::ScopedFlushToZero::new();
        let sr = 48_000.0;
        let (mut a, mut b) = (StereoChorus::new(sr), StereoChorus::new(sr));
        a.set_params(&on(0.6, 0.5, 0.8));
        b.set_params(&on(0.6, 0.5, 0.8));
        for i in 0..2_000 {
            let (l, r) = (sine_01((i as f32 * 220.0 / sr).fract()), 0.3);
            let (mut bl, mut br) = ([l], [r]);
            b.process_block(&mut bl, &mut br);
            assert_eq!(a.process(l, r).0.to_bits(), bl[0].to_bits());
        }
        let off = ChorusParams { on: false, ..on(0.6, 0.5, 0.8) };
        a.set_params(&off);
        b.set_params(&off);
        // Long enough that the 30 ms fade lands mid-run, and in blocks that
        // straddle the settle point. A 30 ms one-pole reaches its snap floor at
        // ~14 tau, so 0.6 s clears it with margin; 48 is deliberately not a
        // divisor of CONTROL_BLOCK, so chunk edges fall mid-fade.
        let mut bl = [0.0f32; 48];
        let mut br = [0.0f32; 48];
        for blk in 0..600 {
            for i in 0..48 {
                bl[i] = sine_01(((blk * 48 + i) as f32 * 220.0 / sr).fract());
                br[i] = 0.3;
            }
            let dry = (bl, br);
            b.process_block(&mut bl, &mut br);
            for i in 0..48 {
                let (al, ar) = a.process(dry.0[i], dry.1[i]);
                assert_eq!(al.to_bits(), bl[i].to_bits(), "L blk{blk} i{i}");
                assert_eq!(ar.to_bits(), br[i].to_bits(), "R blk{blk} i{i}");
            }
        }
        assert!(!b.is_active(), "fade should have settled off by now");
    }

    #[test]
    fn off_from_load_is_bit_exact_from_first_sample() {
        let _ftz = vxn_core_utils::ScopedFlushToZero::new();
        let mut c = StereoChorus::new(48_000.0);
        c.set_params(&ChorusParams { on: false, mix: 0.9, ..ChorusParams::default() });
        test_util::assert_bit_exact_passthrough(|l, r| c.process(l, r), 1_000);
    }

    #[test]
    fn stereo_in_processes_channels_independently() {
        // Sine on L, silence on R: L output must carry the signal, R must be
        // essentially silent. The R line still ticks the noise/LFO machinery
        // but with zero input + zero hiss it contributes nothing audible.
        let _ftz = vxn_core_utils::ScopedFlushToZero::new();
        let sr = 48_000.0;
        let mut c = StereoChorus::new(sr);
        c.set_params(&on(1.0, 0.7, 0.5));

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
                *d = sine_01(phase);
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
        let _ftz = vxn_core_utils::ScopedFlushToZero::new();
        let sr = 48_000.0;
        let mut c = StereoChorus::new(sr);
        // `hiss` is a ChorusParams field since 0229, not a separate setter.
        c.set_params(&ChorusParams { hiss: 1.0, ..on(0.5, 0.5, 1.0) });
        let mut energy = 0.0f32;
        for _ in 0..48_000 {
            let (l, _) = c.process(0.0, 0.0); // silent input
            energy += l.abs();
        }
        assert!(energy > 0.0, "hiss should leak through on silence");
    }
}
