//! Bucket-brigade-device (BBD) modelled delay line — the chorus primitive.
//!
//! Sole consumer is [`crate::chorus`]. What remains is the modulated delay line
//! and the support types it needs ([`DelayBuffer`] cubic/Thiran reads,
//! [`BoundedRandomWalk`] clock jitter, [`OnePoleLpf`]).
//!
//! VXN1 only needs the *short-delay* BBD regime that chorus lives in
//! (1.6–5.4 ms). At those delays the BBD clock runs ~100–300 kHz, far above
//! Nyquist, so no clock-image folding occurs and the sub-sample H-P engine is
//! pure overhead. [`ModDelayLine`] reproduces the BBD's whole transfer
//! function *except* folding — the shared input/output 4-pole banks, soft
//! bucket saturation and clock jitter — at O(1) per sample, sampling each
//! filter bank once per host sample instead of at sub-sample clock ticks.

use crate::flush_denormal;
use crate::math::fast_tanh;
use crate::random_walk::BoundedRandomWalk;
use std::f32::consts::TAU;

// ── Minimal complex f32 ──────────────────────────────────────────────────────

/// Minimal complex-f32 helper — avoids pulling a dependency for one file.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Complex32 {
    pub re: f32,
    pub im: f32,
}

impl Complex32 {
    pub const fn new(re: f32, im: f32) -> Self {
        Self { re, im }
    }
    pub fn conj(self) -> Self {
        Self {
            re: self.re,
            im: -self.im,
        }
    }
    pub fn exp(self) -> Self {
        let m = self.re.exp();
        let (s, c) = self.im.sin_cos();
        Self {
            re: m * c,
            im: m * s,
        }
    }
    /// Multiplicative inverse `1/z`. Undefined at zero.
    pub fn inv(self) -> Self {
        let inv_d = 1.0 / (self.re * self.re + self.im * self.im);
        Self {
            re: self.re * inv_d,
            im: -self.im * inv_d,
        }
    }
}

impl std::ops::Add for Complex32 {
    type Output = Self;
    fn add(self, o: Self) -> Self {
        Self {
            re: self.re + o.re,
            im: self.im + o.im,
        }
    }
}
impl std::ops::Mul for Complex32 {
    type Output = Self;
    fn mul(self, o: Self) -> Self {
        Self {
            re: self.re * o.re - self.im * o.im,
            im: self.re * o.im + self.im * o.re,
        }
    }
}
impl std::ops::Mul<f32> for Complex32 {
    type Output = Self;
    fn mul(self, s: f32) -> Self {
        Self {
            re: self.re * s,
            im: self.im * s,
        }
    }
}
impl std::ops::Div for Complex32 {
    type Output = Self;
    fn div(self, o: Self) -> Self {
        let inv_d = 1.0 / (o.re * o.re + o.im * o.im);
        Self {
            re: (self.re * o.re + self.im * o.im) * inv_d,
            im: (self.im * o.re - self.re * o.im) * inv_d,
        }
    }
}
impl std::ops::Neg for Complex32 {
    type Output = Self;
    fn neg(self) -> Self {
        Self {
            re: -self.re,
            im: -self.im,
        }
    }
}
impl std::ops::AddAssign for Complex32 {
    fn add_assign(&mut self, o: Self) {
        self.re += o.re;
        self.im += o.im;
    }
}

// ── Continuous-time complex pole bank (SoA, vectorisable) ─────────────────────

/// The bank always holds the two conjugate-pole pairs expanded to four complex
/// poles. Fixing the count lets the per-sample loops be `[f32; 4]` arrays the
/// compiler can lower to a single `f32x4` (SSE/NEON 128-bit) lane.
const NPOLES: usize = 4;

/// A bank of complex one-poles `dx/dt = p·x + u(t)`, each advanced once per host
/// sample by the closed-form ODE solution `x[n] = corr·x[n-1] + psi1·u[n]` with
/// `u` held over the sample. The real part of the residue-weighted state sum is
/// the filter output. Real poles come as conjugate pairs so that sum is real.
///
/// Structure-of-arrays layout: every per-pole quantity is a flat `[f32; NPOLES]`
/// so [`advance`](Self::advance) and [`real_output`](Self::real_output) are
/// branch-free fixed-trip loops over contiguous lanes. The four poles are
/// mutually independent within a sample, so this is where the SIMD width lives —
/// the time recurrence itself stays serial (IIR), as it must. Denormal flushing
/// is left to the thread-wide flush-to-zero set at the audio entry, keeping
/// these loops free of the branch a per-lane guard would add.
#[derive(Clone, Debug)]
struct ContinuousPoleBank {
    corr_re: [f32; NPOLES],
    corr_im: [f32; NPOLES],
    psi1_re: [f32; NPOLES],
    psi1_im: [f32; NPOLES],
    r_re: [f32; NPOLES],
    r_im: [f32; NPOLES],
    x_re: [f32; NPOLES],
    x_im: [f32; NPOLES],
}

impl ContinuousPoleBank {
    fn new(poles: [Complex32; NPOLES], residues: [Complex32; NPOLES], sample_rate: f32) -> Self {
        let host_ts = 1.0 / sample_rate;
        let mut b = Self {
            corr_re: [0.0; NPOLES],
            corr_im: [0.0; NPOLES],
            psi1_re: [0.0; NPOLES],
            psi1_im: [0.0; NPOLES],
            r_re: [0.0; NPOLES],
            r_im: [0.0; NPOLES],
            x_re: [0.0; NPOLES],
            x_im: [0.0; NPOLES],
        };
        for k in 0..NPOLES {
            let pole_corr = (poles[k] * host_ts).exp();
            let inv_pole = poles[k].inv();
            let psi1 = Complex32 {
                re: pole_corr.re - 1.0,
                im: pole_corr.im,
            } * inv_pole;
            b.corr_re[k] = pole_corr.re;
            b.corr_im[k] = pole_corr.im;
            b.psi1_re[k] = psi1.re;
            b.psi1_im[k] = psi1.im;
            b.r_re[k] = residues[k].re;
            b.r_im[k] = residues[k].im;
        }
        b
    }

    /// Roll all four poles forward one host sample with input `u` held. The
    /// fixed `0..NPOLES` trip over flat arrays autovectorises to `f32x4`.
    #[inline]
    fn advance(&mut self, u: f32) {
        for k in 0..NPOLES {
            let xr = self.x_re[k];
            let xi = self.x_im[k];
            self.x_re[k] = self.corr_re[k] * xr - self.corr_im[k] * xi + self.psi1_re[k] * u;
            self.x_im[k] = self.corr_re[k] * xi + self.corr_im[k] * xr + self.psi1_im[k] * u;
        }
    }

    fn reset(&mut self) {
        self.x_re = [0.0; NPOLES];
        self.x_im = [0.0; NPOLES];
    }

    /// `Re(Σ r_k · x_k)` — the conjugate pairs make the imaginary part cancel,
    /// so only the real accumulation is computed.
    #[inline]
    fn real_output(&self) -> f32 {
        let mut sum = 0.0_f32;
        for k in 0..NPOLES {
            sum += self.r_re[k] * self.x_re[k] - self.r_im[k] * self.x_im[k];
        }
        sum
    }
}

/// Input / output filter pole set. Two well-damped conjugate-pole pairs giving
/// a non-peaking ~4-pole lowpass rolling off from ~6 kHz. Damped by design so
/// the BBD's input × output transfer stays below unity everywhere. Returns one
/// pole per conjugate pair; the bank adds the twins. Single source of truth —
/// both the input anti-image bank and the output reconstruction bank use it.
fn default_pole_pairs() -> [Complex32; 2] {
    [
        Complex32::new(-30_000.0, 20_000.0),
        Complex32::new(-50_000.0, 30_000.0),
    ]
}

/// Residues (one per pair) normalised so the filter's DC gain is exactly 1.
/// Both raw residues are unit `1+0i`, so `-r/p` reduces to `-1/p` and the
/// doubled-over-halves DC sum collapses to `2·(Re(-1/p₀) + Re(-1/p₁))`.
fn normalised_pair_residues(poles: &[Complex32; 2]) -> [Complex32; 2] {
    let g = 2.0
        * ((-Complex32::new(1.0, 0.0) / poles[0]).re + (-Complex32::new(1.0, 0.0) / poles[1]).re);
    let inv_g = 1.0 / g;
    [Complex32::new(inv_g, 0.0); 2]
}

/// Build a bank carrying the BBD's input/output filter shape: the two pole
/// pairs expanded to full conjugate pairs so the bank's real output is exact.
fn recon_bank(sample_rate: f32) -> ContinuousPoleBank {
    let pairs = default_pole_pairs();
    let res = normalised_pair_residues(&pairs);
    let poles = [pairs[0], pairs[0].conj(), pairs[1], pairs[1].conj()];
    let residues = [res[0], res[0].conj(), res[1], res[1].conj()];
    ContinuousPoleBank::new(poles, residues, sample_rate)
}

// ── One-pole low-pass (trailing reconstruction trim) ──────────────────────────

/// One-pole low-pass: `y[n] = y[n-1] + α (x[n] - y[n-1])`,
/// `α = 1 - exp(-2π fc / sr)`. DC gain unity. The post-BBD reconstruction trim.
#[derive(Default, Clone, Copy)]
pub struct OnePoleLpf {
    alpha: f32,
    y: f32,
}

impl OnePoleLpf {
    pub fn set_cutoff(&mut self, cutoff_hz: f32, sample_rate: f32) {
        self.alpha = 1.0 - (-TAU * cutoff_hz / sample_rate).exp();
    }

    pub fn reset(&mut self) {
        self.y = 0.0;
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        self.y += self.alpha * (x - self.y);
        self.y
    }
}

// ── Power-of-two ring with cubic / Thiran fractional reads ────────────────────

/// Power-of-two circular buffer with cubic and Thiran fractional reads. `push`
/// pre-increments the write head, so a freshly pushed sample sits at offset 0.
#[derive(Clone)]
struct DelayBuffer {
    data: Box<[f32]>,
    mask: usize,
    write: usize,
}

impl DelayBuffer {
    fn for_duration(max_delay_secs: f32, sample_rate: f32) -> Self {
        let min_samples = ((max_delay_secs * sample_rate).ceil() as usize).max(1);
        let size = min_samples.next_power_of_two();
        Self {
            data: vec![0.0; size].into_boxed_slice(),
            mask: size - 1,
            write: 0,
        }
    }

    #[inline]
    fn capacity(&self) -> usize {
        self.mask + 1
    }

    fn clear(&mut self) {
        self.data.iter_mut().for_each(|s| *s = 0.0);
        self.write = 0;
    }

    #[inline]
    fn push(&mut self, sample: f32) {
        self.write = self.write.wrapping_add(1) & self.mask;
        self.data[self.write] = sample;
    }

    #[inline]
    fn read_at(&self, offset: usize) -> f32 {
        self.data[self.write.wrapping_sub(offset) & self.mask]
    }

    /// Catmull-Rom cubic. `offset` in `[0, capacity - 2]`.
    #[inline]
    fn read_cubic(&self, offset: f32) -> f32 {
        let i = offset as usize;
        let f = offset - i as f32;
        let x0 = self.read_at(i.wrapping_sub(1));
        let x1 = self.read_at(i);
        let x2 = self.read_at(i + 1);
        let x3 = self.read_at(i + 2);
        let a0 = -0.5 * x0 + 1.5 * x1 - 1.5 * x2 + 0.5 * x3;
        let a1 = x0 - 2.5 * x1 + 2.0 * x2 - 0.5 * x3;
        let a2 = -0.5 * x0 + 0.5 * x2;
        let a3 = x1;
        ((a0 * f + a1) * f + a2) * f + a3
    }
}

/// First-order Thiran all-pass interpolation state for a [`DelayBuffer`]. Flat
/// magnitude and group delay across the band — the best fractional read for a
/// smoothly modulated line. Recursive, so reset on a discontinuous delay jump.
#[derive(Default, Clone, Copy)]
struct ThiranInterp {
    y_prev: f32,
}

impl ThiranInterp {
    const FRAC_EPSILON: f32 = 1.0e-3;

    fn reset(&mut self) {
        self.y_prev = 0.0;
    }

    #[inline]
    fn read(&mut self, buf: &DelayBuffer, offset: f32) -> f32 {
        let i = offset as usize;
        let frac = (offset - i as f32).clamp(Self::FRAC_EPSILON, 1.0 - Self::FRAC_EPSILON);
        let a = (1.0 - frac) / (1.0 + frac);
        let x0 = buf.read_at(i);
        let x1 = buf.read_at(i + 1);
        let y = a * (x0 - self.y_prev) + x1;
        self.y_prev = y;
        y
    }
}

// ── Fractional-read interpolator selector ─────────────────────────────────────

/// Fractional-read interpolator for the delay tap.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Interp {
    /// Catmull-Rom cubic — stateless, near-flat magnitude.
    #[default]
    Cubic,
    /// First-order Thiran all-pass — flat magnitude + group delay.
    Thiran,
}

// ── Host-rate modulated delay line ────────────────────────────────────────────

const JITTER_MAX_DEPTH: f32 = 0.03;
const JITTER_WALK_INTERVAL: u32 = 64;
const JITTER_WALK_STEP: f32 = 0.03;

/// One host-rate modulated delay line mirroring the BBD chain minus folding:
/// input recon bank → saturating write → fractional read → output recon bank →
/// trailing reconstruction one-pole, with optional clock jitter on the read.
#[derive(Clone)]
pub struct ModDelayLine {
    buf: DelayBuffer,
    input_bank: ContinuousPoleBank,
    output_bank: ContinuousPoleBank,
    recon: OnePoleLpf,
    sample_rate: f32,

    interp: Interp,
    thiran: ThiranInterp,

    sat_drive: f32,
    sat_inv_drive: f32,

    jitter_amount: f32,
    jitter_walk: BoundedRandomWalk,
    jitter_counter: u32,
    jitter_value: f32,
}

impl ModDelayLine {
    /// Allocate a line able to hold up to `max_delay_s` of delay. Two extra
    /// samples of headroom cover the cubic interpolator's upper guard tap.
    pub fn new(max_delay_s: f32, sample_rate: f32) -> Self {
        Self {
            buf: DelayBuffer::for_duration(max_delay_s + 2.0 / sample_rate, sample_rate),
            input_bank: recon_bank(sample_rate),
            output_bank: recon_bank(sample_rate),
            recon: OnePoleLpf::default(),
            sample_rate,
            interp: Interp::default(),
            thiran: ThiranInterp::default(),
            sat_drive: 0.0,
            sat_inv_drive: 1.0,
            jitter_amount: 0.0,
            jitter_walk: BoundedRandomWalk::new(0x1BBD_0001, JITTER_WALK_STEP),
            jitter_counter: 0,
            jitter_value: 0.0,
        }
    }

    /// Set the trailing reconstruction one-pole cutoff (per-variant trim).
    pub fn set_recon_cutoff(&mut self, cutoff_hz: f32) {
        self.recon.set_cutoff(cutoff_hz, self.sample_rate);
    }

    /// Select the fractional-read interpolator. Resets the all-pass state so
    /// the switch itself can't click.
    pub fn set_interp(&mut self, interp: Interp) {
        self.interp = interp;
        self.thiran.reset();
    }

    /// Set write soft-saturation drive. `0.0` disables.
    pub fn set_saturation(&mut self, drive: f32) {
        self.sat_drive = drive.max(0.0);
        self.sat_inv_drive = if self.sat_drive > 0.0 {
            1.0 / self.sat_drive
        } else {
            1.0
        };
    }

    /// Clock-jitter amount in `[0, 1]`. `0.0` disables — the walk is not
    /// advanced and the read delay is used unperturbed.
    pub fn set_jitter_amount(&mut self, amount: f32) {
        self.jitter_amount = amount.clamp(0.0, 1.0);
    }

    /// Seed the jitter walk so sibling lines (stereo chorus) decorrelate.
    pub fn set_jitter_seed(&mut self, seed: u32) {
        self.jitter_walk = BoundedRandomWalk::new(seed, JITTER_WALK_STEP);
        self.jitter_counter = 0;
        self.jitter_value = 0.0;
    }

    pub fn clear(&mut self) {
        self.buf.clear();
        self.input_bank.reset();
        self.output_bank.reset();
        self.recon.reset();
        self.thiran.reset();
        self.jitter_counter = 0;
        self.jitter_value = 0.0;
    }

    /// Process one sample at the commanded delay (seconds). The delay may
    /// change every sample — the fractional read tracks a swept delay cleanly.
    #[inline]
    pub fn process(&mut self, x: f32, delay_s: f32) -> f32 {
        // Clock jitter: slow multiplicative wobble, advanced once per interval.
        let delay_s = if self.jitter_amount > 0.0 {
            if self.jitter_counter == 0 {
                self.jitter_value = self.jitter_walk.advance();
            }
            self.jitter_counter = (self.jitter_counter + 1) % JITTER_WALK_INTERVAL;
            delay_s * (1.0 + self.jitter_value * self.jitter_amount * JITTER_MAX_DEPTH)
        } else {
            delay_s
        };

        // Input anti-aliasing bank → soft-saturating write.
        self.input_bank.advance(x);
        let filtered = self.input_bank.real_output();
        let charge = if self.sat_drive > 0.0 {
            fast_tanh(self.sat_drive * filtered) * self.sat_inv_drive
        } else {
            filtered
        };
        self.buf.push(charge);

        // Fractional read at the commanded delay (offset 0 = freshly pushed).
        let max_offset = self.buf.capacity() as f32 - 2.0;
        let offset = (delay_s * self.sample_rate).clamp(0.0, max_offset);
        let read = match self.interp {
            Interp::Cubic => self.buf.read_cubic(offset),
            Interp::Thiran => self.thiran.read(&self.buf, offset),
        };

        // Output reconstruction bank, then the trailing variant trim.
        self.output_bank.advance(read);
        flush_denormal(self.recon.process(self.output_bank.real_output()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    const SR: f32 = 48_000.0;
    const DELAY_S: f32 = 0.002;

    fn rms_gain<F: FnMut(f32) -> f32>(mut step: F, freq: f32) -> f32 {
        let n = SR as usize;
        let warm = n / 4;
        let (mut si, mut so) = (0.0_f64, 0.0_f64);
        for i in 0..n {
            let x = (TAU * freq * (i as f32 / SR)).sin();
            let y = step(x);
            if i >= warm {
                si += (x as f64).powi(2);
                so += (y as f64).powi(2);
            }
        }
        (so / si).sqrt() as f32
    }

    #[test]
    fn dc_gain_is_unity() {
        // ModDelayLine's banks carry no per-lane denormal flush; they rely on
        // the audio thread running under flush-to-zero. Mirror that here.
        crate::enable_flush_to_zero();
        // Both banks and the one-pole are unity-DC; the delay only shifts.
        let mut line = ModDelayLine::new(0.01, SR);
        line.set_recon_cutoff(8_000.0);
        let mut y = 0.0;
        for _ in 0..(SR as usize) {
            y = line.process(1.0, DELAY_S);
        }
        assert!((y - 1.0).abs() < 1e-3, "DC gain should be ~1.0, got {y}");
    }

    #[test]
    fn passband_is_non_peaking() {
        crate::enable_flush_to_zero();
        for &f in &[50.0, 200.0, 1_000.0, 3_000.0, 6_000.0_f32] {
            let mut line = ModDelayLine::new(0.01, SR);
            line.set_recon_cutoff(8_000.0);
            let g = rms_gain(|x| line.process(x, DELAY_S), f);
            assert!(g <= 1.02, "gain at {f} Hz peaked: {g}");
        }
    }

    #[test]
    fn rolls_off_high_frequencies() {
        crate::enable_flush_to_zero();
        let mut line = ModDelayLine::new(0.01, SR);
        line.set_recon_cutoff(8_000.0);
        let lo = rms_gain(|x| line.process(x, DELAY_S), 1_000.0);
        let mut line = ModDelayLine::new(0.01, SR);
        line.set_recon_cutoff(8_000.0);
        let hi = rms_gain(|x| line.process(x, DELAY_S), 14_000.0);
        assert!(hi < lo * 0.5, "expected HF rolloff: 1k={lo}, 14k={hi}");
    }
}
