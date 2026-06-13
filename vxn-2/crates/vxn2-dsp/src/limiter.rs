//! Stereo lookahead **sample-peak** limiter for the master output bus.
//!
//! Ported verbatim from VXN1 (`vxn-dsp::limiter`). Optional brickwall on the
//! master bus — the engine runs it last in the FX chain (after master gain)
//! when [`crate`]'s `limiter-on` patch flag is set, bypassed otherwise.
//!
//! # Detection: sample-peak, not true-peak
//!
//! Detection is on the base-rate magnitude directly (no 2× oversampling for
//! inter-sample peaks). The output is hard-clamped to ±1, so no *sample* ever
//! exceeds digital full scale regardless; inter-sample overshoot is left to the
//! host / DAC chain, which owns final true-peak compliance. This is an internal
//! master *safety* limiter, not a true-peak mastering meter.
//!
//! # Algorithm
//!
//! Each channel feeds a dry [`DelayLine`]. The base-rate magnitudes are *linked*
//! — `max(|L|, |R|)` is pushed into a single sliding-maximum [`PeakWindow`] — so
//! one gain envelope drives both channels and the stereo image never shifts when
//! only one channel has a transient. The dry signal is read back delayed by the
//! lookahead, so the gain reduction lands *on* the peak it was computed for.

use crate::smoother::{ms_to_samples, one_pole_coeff};

// ── Power-of-two delay line ──────────────────────────────────────────────────

/// Power-of-two circular buffer with fractional (linear-interpolated) reads.
/// Private to the limiter (VXN1's `vxn-dsp::delay::DelayLine`, inlined here —
/// VXN2's `delay.rs` ring isn't a reusable public type).
struct DelayLine {
    buf: Vec<f32>,
    mask: usize,
    write: usize,
}

impl DelayLine {
    /// Allocate a line that can hold at least `max_samples` of delay. Capacity
    /// is rounded up to a power of two. Allocates — call off the audio thread.
    fn new(max_samples: usize) -> Self {
        let cap = max_samples.max(2).next_power_of_two();
        Self {
            buf: vec![0.0; cap],
            mask: cap - 1,
            write: 0,
        }
    }

    fn clear(&mut self) {
        self.buf.iter_mut().for_each(|s| *s = 0.0);
        self.write = 0;
    }

    /// Write one sample at the current head and advance.
    #[inline]
    fn write(&mut self, x: f32) {
        self.buf[self.write] = x;
        self.write = (self.write + 1) & self.mask;
    }

    /// Read `delay_samples` (fractional) behind the most recent write.
    #[inline]
    fn read(&self, delay_samples: f32) -> f32 {
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

// ── Sliding-maximum peak detector ───────────────────────────────────────────

/// Sliding-maximum detector over a configurable window of absolute values.
///
/// Maintains a monotonic deque over a power-of-two ring buffer, giving O(1)
/// amortised `push` and O(1) `peak` regardless of window size. The deque is a
/// fixed array ring (not a `VecDeque`): front/back are plain index arithmetic,
/// so the hot `push` is branch-light with no `Option`/capacity bookkeeping. The
/// deque can never hold more than `buf.len()` indices (one per live slot), so
/// the ring never overflows. All memory is pre-allocated in [`PeakWindow::new`];
/// neither `push` nor `set_window` allocates on the audio thread.
struct PeakWindow {
    buf: Box<[f32]>,
    mask: usize,
    write: usize,
    /// Monotonic deque of indices into `buf`, oldest-max at the front. Backed by
    /// a power-of-two ring sized to `buf.len()`.
    dq: Box<[usize]>,
    dq_mask: usize,
    /// Ring index of the front (oldest) deque entry.
    dq_head: usize,
    /// Live deque length (front is `dq[dq_head]`, back is `dq_head + dq_len - 1`).
    dq_len: usize,
    window: usize,
}

impl PeakWindow {
    /// Allocate a peak window with capacity for at least `min_len` samples
    /// (rounded up to the next power of two). The active window defaults to the
    /// full capacity.
    fn new(min_len: usize) -> Self {
        let size = min_len.max(1).next_power_of_two();
        Self {
            buf: vec![0.0_f32; size].into_boxed_slice(),
            mask: size - 1,
            write: 0,
            dq: vec![0_usize; size].into_boxed_slice(),
            dq_mask: size - 1,
            dq_head: 0,
            dq_len: 0,
            window: size,
        }
    }

    /// Index of the back (newest) deque entry. Caller must ensure `dq_len > 0`.
    #[inline]
    fn dq_back(&self) -> usize {
        self.dq[(self.dq_head + self.dq_len - 1) & self.dq_mask]
    }

    /// Change the active window length without allocating. `n` is clamped to
    /// `[1, capacity]`. Rebuilds the deque in O(capacity) — fine for parameter
    /// changes, not the hot path.
    fn set_window(&mut self, n: usize) {
        self.window = n.clamp(1, self.mask + 1);
        self.rebuild_deque();
    }

    /// Push one sample, advancing the window. Stores the absolute value and
    /// updates the monotonic deque in O(1) amortised.
    #[inline]
    fn push(&mut self, x: f32) {
        self.write = self.write.wrapping_add(1) & self.mask;
        let val = x.abs();
        self.buf[self.write] = val;

        // Evict the front if it has just fallen outside the window.
        let evict = self.write.wrapping_sub(self.window) & self.mask;
        if self.dq_len > 0 && self.dq[self.dq_head] == evict {
            self.dq_head = (self.dq_head + 1) & self.dq_mask;
            self.dq_len -= 1;
        }

        // Maintain the monotone-decreasing invariant: any back entries ≤ the new
        // value can never be the maximum while the new entry is alive.
        while self.dq_len > 0 && self.buf[self.dq_back()] <= val {
            self.dq_len -= 1;
        }
        self.dq[(self.dq_head + self.dq_len) & self.dq_mask] = self.write;
        self.dq_len += 1;
    }

    /// Maximum absolute value over the current window. O(1).
    #[inline]
    fn peak(&self) -> f32 {
        if self.dq_len == 0 {
            0.0
        } else {
            self.buf[self.dq[self.dq_head]]
        }
    }

    fn clear(&mut self) {
        self.buf.iter_mut().for_each(|s| *s = 0.0);
        self.write = 0;
        self.dq_head = 0;
        self.dq_len = 0;
    }

    fn rebuild_deque(&mut self) {
        self.dq_head = 0;
        self.dq_len = 0;
        for age in (0..self.window).rev() {
            let idx = self.write.wrapping_sub(age) & self.mask;
            let val = self.buf[idx];
            while self.dq_len > 0 && self.buf[self.dq_back()] <= val {
                self.dq_len -= 1;
            }
            self.dq[(self.dq_head + self.dq_len) & self.dq_mask] = idx;
            self.dq_len += 1;
        }
    }
}

// ── Gain-envelope core ──────────────────────────────────────────────────────

/// Shared gain-envelope core: peak tracking, target-gain computation, and
/// smoothed gain reduction with separate attack/release coefficients.
struct LimiterCore {
    peak_window: PeakWindow,
    current_gain: f32,
    lookahead_samples: usize,
    threshold_internal: f32,
    attack_coeff: f32,
    release_coeff: f32,
}

impl LimiterCore {
    fn new(
        sample_rate: f32,
        threshold: f32,
        attack_ms: f32,
        release_ms: f32,
        max_attack_ms: f32,
    ) -> Self {
        let lookahead_samples = ms_to_samples(attack_ms, sample_rate);
        let max_lookahead = ms_to_samples(max_attack_ms, sample_rate);
        // Base-rate detection: one magnitude per sample, window = lookahead + 1.
        let mut peak_window = PeakWindow::new(max_lookahead + 1);
        peak_window.set_window(lookahead_samples + 1);
        Self {
            peak_window,
            current_gain: 1.0,
            lookahead_samples,
            // Hold a hair under the threshold so the envelope settles before the
            // peak actually arrives (matches the reference limiter).
            threshold_internal: threshold.max(0.0) * 0.98,
            attack_coeff: one_pole_coeff(attack_ms, sample_rate),
            release_coeff: one_pole_coeff(release_ms, sample_rate),
        }
    }

    fn set_threshold(&mut self, threshold: f32) {
        self.threshold_internal = threshold.max(0.0) * 0.98;
    }

    /// Push the base-rate magnitude into the peak window. Call once per sample.
    #[inline]
    fn push_magnitude(&mut self, magnitude: f32) {
        self.peak_window.push(magnitude);
    }

    /// Update the smoothed gain after this sample's magnitudes are all pushed.
    #[inline]
    fn update_gain(&mut self) {
        let peak = self.peak_window.peak();
        let target_gain = if peak > self.threshold_internal {
            (self.threshold_internal / peak).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let coeff = if target_gain < self.current_gain {
            self.attack_coeff
        } else {
            self.release_coeff
        };
        self.current_gain += coeff * (target_gain - self.current_gain);
    }

    #[inline]
    fn current_gain(&self) -> f32 {
        self.current_gain
    }

    /// Read offset for the dry delay line: the lookahead, so the gain reduction
    /// aligns with the peak it anticipates.
    #[inline]
    fn read_offset(&self) -> usize {
        self.lookahead_samples
    }

    fn reset(&mut self) {
        self.peak_window.clear();
        self.current_gain = 1.0;
    }
}

// ── Stereo wrapper ──────────────────────────────────────────────────────────

/// Threshold (linear, pre 0.98 trim) for the master-bus limiter ceiling.
const THRESHOLD: f32 = 0.95;
/// Attack / lookahead time in ms.
const ATTACK_MS: f32 = 2.0;
/// Release time in ms.
const RELEASE_MS: f32 = 100.0;
/// Maximum attack used to size the lookahead buffers.
const MAX_ATTACK_MS: f32 = 50.0;

/// Stereo lookahead peak limiter with linked L/R sidechain.
///
/// See the [module docs](self) for the algorithm. Process one sample at a time
/// with [`process`](Self::process), or a whole block with
/// [`process_block`](Self::process_block); the engine calls it on the master
/// bus when the limiter is enabled.
pub struct StereoLimiter {
    dry_l: DelayLine,
    dry_r: DelayLine,
    core: LimiterCore,
}

impl StereoLimiter {
    /// Build a master-bus limiter at `sample_rate` with the fixed master ceiling
    /// defaults (threshold ≈ −0.4 dBFS, 2 ms lookahead, 100 ms release).
    pub fn new(sample_rate: f32) -> Self {
        let core = LimiterCore::new(sample_rate, THRESHOLD, ATTACK_MS, RELEASE_MS, MAX_ATTACK_MS);
        // Cover the largest read offset the lookahead window can demand.
        let delay_len = ms_to_samples(MAX_ATTACK_MS, sample_rate) + 1;
        Self {
            dry_l: DelayLine::new(delay_len),
            dry_r: DelayLine::new(delay_len),
            core,
        }
    }

    /// Update the limiting threshold (linear, before the internal 0.98 trim).
    pub fn set_threshold(&mut self, threshold: f32) {
        self.core.set_threshold(threshold);
    }

    /// Clear all delay/filter state and reset the gain to unity. Call when the
    /// limiter is (re)engaged or the transport resets, so stale lookahead samples
    /// don't leak a transient into the first block.
    pub fn reset(&mut self) {
        self.dry_l.clear();
        self.dry_r.clear();
        self.core.reset();
    }

    /// Limit a stereo block in place. The core is a serial recurrence (peak
    /// window + gain smoothing), so this can't vectorise; it loops [`process`]
    /// but keeps the gain envelope state in registers across the block and lets
    /// the caller drop the per-sample call/borrow. `l` and `r` must be equal
    /// length; the shorter bound is processed.
    #[inline]
    pub fn process_block(&mut self, l: &mut [f32], r: &mut [f32]) {
        let n = l.len().min(r.len());
        for (ls, rs) in l[..n].iter_mut().zip(r[..n].iter_mut()) {
            let (lo, ro) = self.process(*ls, *rs);
            *ls = lo;
            *rs = ro;
        }
    }

    /// Limit one stereo sample, returning the gain-reduced, ceiling-clamped pair.
    #[inline]
    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        self.dry_l.write(l);
        self.dry_r.write(r);

        // Linked sidechain: feed the detector the base-rate max(|L|, |R|).
        self.core.push_magnitude(l.abs().max(r.abs()));
        self.core.update_gain();

        let offset = self.core.read_offset() as f32;
        let gain = self.core.current_gain();
        let dl = self.dry_l.read(offset);
        let dr = self.dry_r.read(offset);
        ((dl * gain).clamp(-1.0, 1.0), (dr * gain).clamp(-1.0, 1.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn warmup(limiter: &mut StereoLimiter, l: f32, r: f32, n: usize) {
        for _ in 0..n {
            limiter.process(l, r);
        }
    }

    #[test]
    fn below_threshold_is_near_unity() {
        let mut lim = StereoLimiter::new(SR);
        warmup(&mut lim, 0.5, 0.3, 256);
        let (l, r) = lim.process(0.5, 0.3);
        assert!((l - 0.5).abs() < 0.05, "left {l} not ~0.5");
        assert!((r - 0.3).abs() < 0.05, "right {r} not ~0.3");
    }

    #[test]
    fn output_bounded_above_threshold() {
        let mut lim = StereoLimiter::new(SR);
        // Slam well past the ceiling; output must stay bounded by the threshold.
        for _ in 0..24_000 {
            let (l, r) = lim.process(2.0, 2.0);
            assert!(
                l.abs() <= 1.0 && r.abs() <= 1.0,
                "output exceeds ±1: {l}, {r}"
            );
        }
        let (l, _) = lim.process(2.0, 2.0);
        assert!(
            l.abs() < 1.0,
            "settled output {l} should sit under the ceiling"
        );
    }

    #[test]
    fn linked_sidechain_reduces_quiet_channel() {
        let mut lim = StereoLimiter::new(SR);
        // Drive only left hard; the linked gain should pull the quiet right down.
        for _ in 0..24_000 {
            lim.process(2.0, 0.4);
        }
        let (_, r) = lim.process(2.0, 0.4);
        assert!(r.abs() < 0.35, "right {r} not reduced by linked sidechain");
    }

    #[test]
    fn stereo_image_preserved() {
        let mut lim = StereoLimiter::new(SR);
        for _ in 0..24_000 {
            lim.process(1.0, 1.0);
        }
        let (l, r) = lim.process(1.0, 1.0);
        assert!((l - r).abs() < 0.001, "image shifted: L={l} R={r}");
    }

    #[test]
    fn recovers_after_transient() {
        let mut lim = StereoLimiter::new(SR);
        for _ in 0..4_000 {
            lim.process(2.0, 2.0);
        }
        for _ in 0..48_000 {
            lim.process(0.0, 0.0);
        }
        // After a long release into silence, a quiet probe passes near unity.
        warmup(&mut lim, 0.3, 0.3, 256);
        let (l, _) = lim.process(0.3, 0.3);
        assert!(
            l.abs() > 0.27,
            "probe over-attenuated ({l}); gain not recovered"
        );
    }

    #[test]
    fn reset_clears_state() {
        let mut lim = StereoLimiter::new(SR);
        for _ in 0..4_000 {
            lim.process(2.0, 2.0);
        }
        lim.reset();
        assert_eq!(lim.core.current_gain(), 1.0);
        // A fresh quiet signal is delayed but finite and bounded after reset.
        for _ in 0..256 {
            let (l, r) = lim.process(0.2, 0.2);
            assert!(l.is_finite() && r.is_finite());
        }
    }

    #[test]
    fn output_finite_under_noise() {
        let mut lim = StereoLimiter::new(SR);
        let mut x = 0x1234_5678u64;
        for _ in 0..48_000 {
            // cheap LCG noise in [-2, 2]
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
            let n = ((x >> 33) as f32 / u32::MAX as f32) * 4.0 - 2.0;
            let (l, r) = lim.process(n, -n);
            assert!(l.is_finite() && r.is_finite(), "non-finite output");
            assert!(l.abs() <= 1.0 && r.abs() <= 1.0, "output out of range");
        }
    }
}
