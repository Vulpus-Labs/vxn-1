//! 8-channel FDN reverb, ported from `vxn-2/crates/vxn2-dsp/src/reverb.rs`.
//!
//! Clean Jot-style Feedback Delay Network: 8 mutually-prime delay lines, an
//! 8×8 Hadamard mixing matrix on the feedback path, per-line one-pole LP
//! damping, and ±2-sample LFO modulation on each line for flutter
//! suppression.
//!
//! ## Macros
//!
//! - `size` scales the base delay-line lengths (smoothed over ~500 ms so
//!   size sweeps glide rather than click).
//! - `decay_secs` is the RT60 target. The per-line feedback gain is derived
//!   from the standard FDN formula `g = 10^(-3·L / (decay·sr))`.
//! - `damp` drives the one-pole LP cutoff on each delay-line output (higher
//!   damp → lower cutoff → faster HF decay).
//! - `mix` is the wet/dry crossfade (`(1-mix)·dry + mix·wet`).
//!
//! ## Stereo image
//!
//! `in_l` feeds lines 0..3, `in_r` feeds lines 4..7, each through a fixed
//! ±1 sign pattern. Cross-feedback via Hadamard mixes channels, then
//! channels 0..3 sum to L out and 4..7 sum to R out.
//!
//! ## Bypass
//!
//! `on = false` returns `(in_l, in_r)` bit-identical with no buffer work.

use crate::Smoothed;

/// Number of delay lines.
pub const LINES: usize = 8;
/// Maximum `size` scale factor (multiplies BASE_MS).
const MAX_SIZE_SCALE: f32 = 2.0;
/// Lower bound for the size scale.
const MIN_SIZE_SCALE: f32 = 0.2;
/// Glide time for the size scale smoother. Size changes glide rather than
/// snap, so re-deriving delay lengths is audibly a crossfade not a click.
const SIZE_SMOOTH_MS: f32 = 500.0;
/// LFO frequency on each delay line, Hz.
const LFO_HZ: f32 = 0.5;
/// LFO depth in samples (peak deviation around the base length).
const LFO_DEPTH_SAMP: f32 = 2.0;
/// 1/√8 — Hadamard normalisation. Without it the matrix multiplies energy
/// by 8 per pass; with it the matrix is unitary.
const INV_SQRT8: f32 = 0.353_553_4_f32;

/// Mutually-prime base delay-line lengths in milliseconds (Jot's canonical
/// 8-line set). Scaled by `size` at runtime.
const BASE_MS: [f32; LINES] = [29.7, 37.1, 41.1, 43.7, 53.3, 59.7, 67.1, 79.3];

/// Fixed ±1 input-gain pattern. Random-looking but deterministic so two
/// instances render bit-identically.
const INPUT_SIGN: [f32; LINES] = [1.0, -1.0, 1.0, 1.0, -1.0, 1.0, -1.0, -1.0];

// ─── Ring (AoS — one row per sample) ─────────────────────────────────────────

/// Interleaved ring buffer: each time slot holds one `[f32; LINES]` row.
/// A per-sample push is one 32-byte contiguous store. Reads are scattered
/// (independent fractional offset per line) so this layout is read-neutral
/// but write-friendly.
struct InterleavedRing {
    data: Box<[[f32; LINES]]>,
    mask: usize,
    write: usize,
}

impl InterleavedRing {
    fn new(min_samples: usize) -> Self {
        let size = min_samples.next_power_of_two().max(2);
        Self {
            data: vec![[0.0_f32; LINES]; size].into_boxed_slice(),
            mask: size - 1,
            write: 0,
        }
    }

    #[inline]
    fn capacity(&self) -> usize {
        self.mask + 1
    }

    #[inline]
    fn push(&mut self, row: [f32; LINES]) {
        self.write = self.write.wrapping_add(1) & self.mask;
        self.data[self.write] = row;
    }

    /// Linear interpolation. `offset ∈ [1.0, capacity() - 2.0]`.
    #[inline]
    fn read_linear(&self, i: usize, offset: f32) -> f32 {
        let k = offset as usize;
        let f = offset - k as f32;
        let a = self.data[self.write.wrapping_sub(k) & self.mask][i];
        let b = self.data[self.write.wrapping_sub(k + 1) & self.mask][i];
        a + f * (b - a)
    }

    fn clear(&mut self) {
        for row in self.data.iter_mut() {
            *row = [0.0; LINES];
        }
        self.write = 0;
    }
}

// ─── Hadamard ────────────────────────────────────────────────────────────────

/// 8×8 fast Walsh-Hadamard transform: 24 add/sub, no multiplies. Output is
/// scaled by `1/√8` to make it unitary.
#[inline]
fn hadamard8(mut x: [f32; LINES]) -> [f32; LINES] {
    for step in [4_usize, 2, 1] {
        let mut i = 0;
        while i < LINES {
            for j in i..i + step {
                let a = x[j];
                let b = x[j + step];
                x[j] = a + b;
                x[j + step] = a - b;
            }
            i += step * 2;
        }
    }
    let g = INV_SQRT8;
    for v in x.iter_mut() {
        *v *= g;
    }
    x
}

// ─── Params ──────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug)]
pub struct FdnReverbParams {
    pub on: bool,
    /// 0.0 ..= 1.0 (clamped). Maps to a delay-length scale.
    pub size: f32,
    /// RT60 target, seconds. Drives feedback gain.
    pub decay_secs: f32,
    /// 0.0 ..= 1.0 (clamped). 0 = no HF roll-off; 1 = aggressive damp.
    pub damp: f32,
    /// 0.0 ..= 1.0 (clamped). Linear `(1-mix)·dry + mix·wet`.
    pub mix: f32,
}

impl Default for FdnReverbParams {
    fn default() -> Self {
        Self {
            on: true,
            size: 0.55,
            decay_secs: 2.4,
            damp: 0.50,
            mix: 0.20,
        }
    }
}

/// FDN reverb with size smoothing, per-line LFO modulation, per-line
/// one-pole damping, and Hadamard feedback mixing.
pub struct FdnReverb {
    sr: f32,
    sr_recip: f32,

    ring: InterleavedRing,
    max_offset: f32,

    base_samps: [f32; LINES],
    size: Smoothed,

    lfo_phase: [f32; LINES],
    lfo_inc: f32,

    damp_y: [f32; LINES],
    damp_a: f32,

    feedback: f32,

    mix: f32,
    on: bool,
}

impl FdnReverb {
    pub fn new(sample_rate: f32) -> Self {
        let sr = sample_rate;
        let sr_recip = sr.recip();
        let base_samps = std::array::from_fn(|i| BASE_MS[i] * 0.001 * sr);

        let max_base = base_samps[LINES - 1] * MAX_SIZE_SCALE + LFO_DEPTH_SAMP + 4.0;
        let ring = InterleavedRing::new(max_base.ceil() as usize);
        let max_offset = (ring.capacity() as f32 - 2.0).max(2.0);

        let p = FdnReverbParams::default();
        let init_scale = scale_from_size(p.size);
        let mut size = Smoothed::new(init_scale, SIZE_SMOOTH_MS, sr);
        size.snap(init_scale);

        let lfo_phase = std::array::from_fn(|i| i as f32 / LINES as f32);
        let lfo_inc = LFO_HZ * sr_recip;

        let mut r = Self {
            sr,
            sr_recip,
            ring,
            max_offset,
            base_samps,
            size,
            lfo_phase,
            lfo_inc,
            damp_y: [0.0; LINES],
            damp_a: 0.0,
            feedback: 0.0,
            mix: p.mix.clamp(0.0, 1.0),
            on: p.on,
        };
        r.update_damp(p.damp);
        r.update_feedback(p.decay_secs, init_scale);
        r
    }

    pub fn set_params(&mut self, p: &FdnReverbParams) {
        self.on = p.on;
        self.mix = p.mix.clamp(0.0, 1.0);

        let target_scale = scale_from_size(p.size);
        self.size.set_target(target_scale);

        self.update_damp(p.damp);
        self.update_feedback(p.decay_secs, target_scale);
    }

    fn update_damp(&mut self, damp01: f32) {
        let d = damp01.clamp(0.0, 1.0);
        // 20 kHz at damp=0 → 500 Hz at damp=1, log-spaced.
        let fc = 20_000.0 * (500.0_f32 / 20_000.0).powf(d);
        // One-pole LP coefficient: `a = exp(-2π·fc/sr)` such that
        // `y = (1-a)·x + a·y_prev`.
        self.damp_a = (-(std::f32::consts::TAU) * fc * self.sr_recip).exp();
    }

    fn update_feedback(&mut self, decay_secs: f32, scale: f32) {
        let l_avg: f32 = self.base_samps.iter().sum::<f32>() / LINES as f32 * scale;
        let decay = decay_secs.max(0.1);
        // g = 10^(-3·L / (decay·sr)). Clamp at 0.999 to keep the tail bounded
        // even when the user asks for "infinite" decay.
        let g = 10.0_f32.powf(-3.0 * l_avg / (decay * self.sr));
        self.feedback = g.clamp(0.0, 0.999);
    }

    /// Process one stereo sample. When `on = false` returns `(in_l, in_r)`
    /// bit-identical and does no buffer work.
    #[inline]
    pub fn process(&mut self, in_l: f32, in_r: f32) -> (f32, f32) {
        if !self.on {
            return (in_l, in_r);
        }

        let scale = self.size.tick();
        let max_off = self.max_offset;

        // ── Read taps with LFO-modulated offsets ─────────────────────────
        let mut tap = [0.0_f32; LINES];
        for i in 0..LINES {
            // Bhaskara-style sine of 2π·phase via piecewise polynomial.
            let p = self.lfo_phase[i] - 0.5;
            let s1 = p * 16.0 * (p.abs() - 0.5);
            let s = s1 + 0.225 * s1 * (s1.abs() - 1.0);
            let off = (self.base_samps[i] * scale + LFO_DEPTH_SAMP * s).clamp(1.0, max_off);
            tap[i] = self.ring.read_linear(i, off);

            let next = self.lfo_phase[i] + self.lfo_inc;
            self.lfo_phase[i] = if next >= 1.0 { next - 1.0 } else { next };
        }

        // ── Per-line one-pole LP damping ─────────────────────────────────
        let a = self.damp_a;
        let mut damp = [0.0_f32; LINES];
        for i in 0..LINES {
            let y = (1.0 - a) * tap[i] + a * self.damp_y[i];
            self.damp_y[i] = y;
            damp[i] = y;
        }

        // ── Hadamard feedback mix ────────────────────────────────────────
        let mixed = hadamard8(damp);

        // ── Inject input (L → 0..3, R → 4..7, signed) + feedback ─────────
        let mut new_row = [0.0_f32; LINES];
        for i in 0..LINES {
            let inj = if i < LINES / 2 { in_l } else { in_r };
            new_row[i] = INPUT_SIGN[i] * inj + self.feedback * mixed[i];
        }
        self.ring.push(new_row);

        // ── Stereo wet sum: L = sum(0..3), R = sum(4..7) ─────────────────
        let wet_l = 0.5 * (damp[0] + damp[1] + damp[2] + damp[3]);
        let wet_r = 0.5 * (damp[4] + damp[5] + damp[6] + damp[7]);

        let mix = self.mix;
        let dry = 1.0 - mix;
        (dry * in_l + mix * wet_l, dry * in_r + mix * wet_r)
    }

    /// Block process: stereo in, stereo out. Engine bus shape (post-delay,
    /// pre-limiter). Loops the per-sample core; FDN's per-sample serial
    /// dependencies don't vectorise across samples.
    pub fn process_block(
        &mut self,
        in_l: &[f32],
        in_r: &[f32],
        out_l: &mut [f32],
        out_r: &mut [f32],
    ) {
        let n = in_l.len().min(in_r.len()).min(out_l.len()).min(out_r.len());
        for i in 0..n {
            let (l, r) = self.process(in_l[i], in_r[i]);
            out_l[i] = l;
            out_r[i] = r;
        }
    }

    /// Zero buffers + filter state. Smoother target preserved.
    pub fn reset(&mut self) {
        self.ring.clear();
        self.damp_y = [0.0; LINES];
        self.lfo_phase = std::array::from_fn(|i| i as f32 / LINES as f32);
    }

    pub fn buffer_capacity(&self) -> usize {
        self.ring.capacity()
    }
}

#[inline]
fn scale_from_size(size01: f32) -> f32 {
    let s = size01.clamp(0.0, 1.0);
    MIN_SIZE_SCALE + s * (MAX_SIZE_SCALE - MIN_SIZE_SCALE)
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn make() -> FdnReverb {
        FdnReverb::new(SR)
    }

    #[test]
    fn buffer_holds_max_size_at_sr() {
        let r = FdnReverb::new(96_000.0);
        let max_samples = BASE_MS[LINES - 1] * 0.001 * 96_000.0 * MAX_SIZE_SCALE;
        assert!(r.buffer_capacity() as f32 >= max_samples);
        assert!(r.buffer_capacity().is_power_of_two());
    }

    #[test]
    fn bypass_passes_input_bit_identical() {
        let mut r = make();
        let p = FdnReverbParams {
            on: false,
            ..Default::default()
        };
        r.set_params(&p);
        for n in 0..2048 {
            let l = (n as f32 * 0.001).sin();
            let rs = (n as f32 * 0.0017).cos();
            let (ol, or_) = r.process(l, rs);
            assert_eq!(ol, l, "L not bit-identical at n={n}");
            assert_eq!(or_, rs, "R not bit-identical at n={n}");
        }
    }

    #[test]
    fn hadamard_is_unitary() {
        let x = [1.0, -0.5, 0.25, 0.75, -1.0, 0.3, -0.4, 0.9];
        let y = hadamard8(x);
        let nx: f32 = x.iter().map(|v| v * v).sum();
        let ny: f32 = y.iter().map(|v| v * v).sum();
        assert!((nx - ny).abs() < 1e-5, "‖x‖²={nx} ‖H·x‖²={ny}");
    }

    #[test]
    fn hadamard_is_involution_up_to_sign() {
        let x = [1.0, -0.5, 0.25, 0.75, -1.0, 0.3, -0.4, 0.9];
        let y = hadamard8(hadamard8(x));
        for i in 0..LINES {
            assert!((x[i] - y[i]).abs() < 1e-5, "i={i}: {} vs {}", x[i], y[i]);
        }
    }

    #[test]
    fn impulse_produces_diffuse_tail() {
        let mut r = make();
        let p = FdnReverbParams {
            on: true,
            size: 0.5,
            decay_secs: 1.5,
            damp: 0.0,
            mix: 1.0,
        };
        r.set_params(&p);
        for _ in 0..(SR as usize) {
            let _ = r.process(0.0, 0.0);
        }
        let _ = r.process(1.0, 1.0);
        let mut peak = 0.0_f32;
        let mut rms = 0.0_f32;
        let n = (0.5 * SR) as usize;
        for _ in 0..n {
            let (l, rr) = r.process(0.0, 0.0);
            peak = peak.max(l.abs()).max(rr.abs());
            rms += l * l + rr * rr;
        }
        rms = (rms / (2.0 * n as f32)).sqrt();
        assert!(peak.is_finite(), "tail diverged");
        assert!(peak > 0.0, "no tail produced");
        assert!(peak < 2.0, "tail too hot, peak={peak}");
        assert!(rms > 1e-4, "tail too quiet, rms={rms}");
    }

    #[test]
    fn longer_decay_lasts_longer() {
        fn tail_energy(decay: f32) -> f32 {
            let mut r = make();
            let p = FdnReverbParams {
                on: true,
                size: 0.5,
                decay_secs: decay,
                damp: 0.0,
                mix: 1.0,
            };
            r.set_params(&p);
            for _ in 0..(SR as usize) {
                let _ = r.process(0.0, 0.0);
            }
            let _ = r.process(1.0, 1.0);
            for _ in 0..((0.2 * SR) as usize) {
                let _ = r.process(0.0, 0.0);
            }
            let mut e = 0.0_f32;
            for _ in 0..((0.1 * SR) as usize) {
                let (l, rr) = r.process(0.0, 0.0);
                e += l * l + rr * rr;
            }
            e
        }
        let short = tail_energy(0.3);
        let long = tail_energy(8.0);
        assert!(
            long > short * 5.0,
            "long decay ({long}) should dwarf short ({short})"
        );
    }

    #[test]
    fn damp_attenuates_hf() {
        fn rms_with_damp(damp: f32) -> f32 {
            let mut r = make();
            let p = FdnReverbParams {
                on: true,
                size: 0.5,
                decay_secs: 2.0,
                damp,
                mix: 1.0,
            };
            r.set_params(&p);
            for n in 0..(SR as usize / 2) {
                let t = n as f32 / SR;
                let s = (t * 8000.0 * std::f32::consts::TAU).sin();
                let _ = r.process(s, s);
            }
            let mut e = 0.0_f32;
            let nn = (0.1 * SR) as usize;
            for n in 0..nn {
                let t = (SR as usize / 2 + n) as f32 / SR;
                let s = (t * 8000.0 * std::f32::consts::TAU).sin();
                let (l, rr) = r.process(s, s);
                e += l * l + rr * rr;
            }
            (e / (2.0 * nn as f32)).sqrt()
        }
        let bright = rms_with_damp(0.0);
        let dark = rms_with_damp(1.0);
        assert!(
            dark < bright * 0.8,
            "damp didn't bite: bright={bright} dark={dark}"
        );
    }

    #[test]
    fn mix_zero_is_dry() {
        let mut r = make();
        let p = FdnReverbParams {
            on: true,
            mix: 0.0,
            ..Default::default()
        };
        r.set_params(&p);
        for _ in 0..1024 {
            let _ = r.process(0.3, -0.2);
        }
        let (l, rr) = r.process(0.42, -0.17);
        assert!((l - 0.42).abs() < 1e-6, "mix=0 L: {l}");
        assert!((rr + 0.17).abs() < 1e-6, "mix=0 R: {rr}");
    }

    #[test]
    fn feedback_capped_below_unity() {
        let mut r = make();
        let p = FdnReverbParams {
            on: true,
            decay_secs: 1_000.0,
            mix: 1.0,
            ..Default::default()
        };
        r.set_params(&p);
        let _ = r.process(1.0, 1.0);
        let mut peak = 0.0_f32;
        for _ in 0..(SR as usize * 3) {
            let (l, rr) = r.process(0.0, 0.0);
            peak = peak.max(l.abs()).max(rr.abs());
        }
        assert!(peak.is_finite(), "feedback exploded");
        assert!(peak < 5.0, "feedback unbounded, peak={peak}");
    }

    #[test]
    fn stereo_input_produces_stereo_image() {
        let mut r = make();
        let p = FdnReverbParams {
            on: true,
            size: 0.3,
            decay_secs: 1.0,
            damp: 0.0,
            mix: 1.0,
        };
        r.set_params(&p);
        for _ in 0..(SR as usize) {
            let _ = r.process(0.0, 0.0);
        }
        let _ = r.process(1.0, 0.0);
        let mut l_e = 0.0_f32;
        let mut r_e = 0.0_f32;
        for _ in 0..((0.1 * SR) as usize) {
            let (l, rr) = r.process(0.0, 0.0);
            l_e += l * l;
            r_e += rr * rr;
        }
        assert!(l_e > 1e-5, "L wet silent (L_e={l_e})");
        assert!(r_e > 1e-5, "R wet silent (R_e={r_e})");
    }

    #[test]
    fn reset_zeros_state() {
        let mut r = make();
        r.set_params(&FdnReverbParams {
            mix: 1.0,
            ..Default::default()
        });
        for _ in 0..10_000 {
            let _ = r.process(0.5, -0.3);
        }
        r.reset();
        let (l, rr) = r.process(0.0, 0.0);
        assert_eq!(l, 0.0);
        assert_eq!(rr, 0.0);
    }

    #[test]
    fn block_matches_per_sample() {
        let mut a = make();
        let mut b = make();
        let p = FdnReverbParams::default();
        a.set_params(&p);
        b.set_params(&p);
        let mut in_l = [0f32; 64];
        let mut in_r = [0f32; 64];
        let mut bl = [0f32; 64];
        let mut br = [0f32; 64];
        for n in 0..in_l.len() {
            let t = n as f32 / SR;
            in_l[n] = (t * 330.0 * std::f32::consts::TAU).sin() * 0.4;
            in_r[n] = (t * 440.0 * std::f32::consts::TAU).sin() * 0.4;
        }
        b.process_block(&in_l, &in_r, &mut bl, &mut br);
        for i in 0..in_l.len() {
            let (l, r) = a.process(in_l[i], in_r[i]);
            assert!((l - bl[i]).abs() < 1e-6, "L mismatch i={i}: {l} vs {}", bl[i]);
            assert!((r - br[i]).abs() < 1e-6, "R mismatch i={i}: {r} vs {}", br[i]);
        }
    }
}
