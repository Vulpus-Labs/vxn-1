//! Stereo BPM-syncable delay (ticket 0010 / ADR §7).
//!
//! Clean delay — no tape, no filter on the feedback path beyond a one-pole DC
//! blocker at ~10 Hz. Character lives in the synth, not the FX.
//!
//! Two independent mono delay lines (L + R), each a power-of-two ring buffer
//! sized for [`MAX_DELAY_S`] at the prepared sample rate. Buffers are allocated
//! once at construction; only a sample-rate change forces re-allocation.
//!
//! ## Routing
//!
//! - Straight: `buf_L` stores `in_L + fb·tap_L`; `buf_R` stores `in_R + fb·tap_R`.
//! - Ping-pong: full crossfeed — `in_L` writes into `buf_R` and feedback
//!   bounces L↔R every delay period.
//!
//! ## DC blocker
//!
//! One-pole highpass on the feedback path only (not on the dry sum). Without
//! it, asymmetric tanh-style FM output accumulates a DC offset around the
//! feedback loop. Cutoff is fixed at ~10 Hz.
//!
//! ## Smoothed delay length
//!
//! Delay time is stored as a smoothed sample count with a ~100 ms glide.
//! Abrupt changes to the read position pitch-shift-click on tempo changes;
//! the smoother avoids that. The tap reader uses Catmull-Rom cubic
//! interpolation on fractional sample positions.

use crate::lfo::SUBDIVISIONS;
use crate::smoother::Smoothed;

/// Maximum delay time in seconds (sets buffer capacity).
pub const MAX_DELAY_S: f32 = 4.0;
/// Lower bound on delay time. Avoids zero-length reads and stays clear of
/// the cubic-interp guard taps.
pub const MIN_DELAY_MS: f32 = 1.0;
/// Upper bound on `time_ms` (matches `MAX_DELAY_S`).
pub const MAX_DELAY_MS: f32 = 4000.0;
/// Hard cap on feedback to prevent runaway.
pub const MAX_FEEDBACK: f32 = 0.95;

const SMOOTH_MS: f32 = 100.0;
/// Dry/wet glide time — masks a mix-knob jump and fades the wet up from 0 on
/// switch-on so the delay doesn't click in at full level.
const MIX_SMOOTH_MS: f32 = 30.0;
const DC_FC_HZ: f32 = 10.0;

// ─── Ring buffer ─────────────────────────────────────────────────────────────

struct Ring {
    data: Box<[f32]>,
    mask: usize,
    write: usize,
}

impl Ring {
    fn new(min_samples: usize) -> Self {
        let size = min_samples.next_power_of_two().max(2);
        Self {
            data: vec![0.0_f32; size].into_boxed_slice(),
            mask: size - 1,
            write: 0,
        }
    }

    #[inline]
    fn capacity(&self) -> usize {
        self.mask + 1
    }

    #[inline]
    fn push(&mut self, x: f32) {
        self.write = self.write.wrapping_add(1) & self.mask;
        self.data[self.write] = x;
    }

    #[inline]
    fn read_at(&self, offset: usize) -> f32 {
        self.data[self.write.wrapping_sub(offset) & self.mask]
    }

    /// Catmull-Rom cubic. `offset` must be in `[1.0, capacity() - 2.0]`.
    #[inline]
    fn read_cubic(&self, offset: f32) -> f32 {
        let i = offset as usize;
        let f = offset - i as f32;
        let x0 = self.read_at(i.wrapping_sub(1));
        let x1 = self.read_at(i);
        let x2 = self.read_at(i + 1);
        let x3 = self.read_at(i + 2);
        let f2 = f * f;
        let f3 = f2 * f;
        let w0 = 0.5 * (-f3 + 2.0 * f2 - f);
        let w1 = 0.5 * (3.0 * f3 - 5.0 * f2 + 2.0);
        let w2 = 0.5 * (-3.0 * f3 + 4.0 * f2 + f);
        let w3 = 0.5 * (f3 - f2);
        w0 * x0 + w1 * x1 + w2 * x2 + w3 * x3
    }

    fn clear(&mut self) {
        for x in self.data.iter_mut() {
            *x = 0.0;
        }
        self.write = 0;
    }
}

// ─── DC blocker ──────────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
struct DcBlock {
    x1: f32,
    y1: f32,
    r: f32,
}

impl DcBlock {
    fn new(sample_rate: f32, fc_hz: f32) -> Self {
        let r = 1.0 - (2.0 * std::f32::consts::PI * fc_hz / sample_rate);
        Self {
            x1: 0.0,
            y1: 0.0,
            r,
        }
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let y = x - self.x1 + self.r * self.y1;
        self.x1 = x;
        self.y1 = y;
        y
    }

    fn reset(&mut self) {
        self.x1 = 0.0;
        self.y1 = 0.0;
    }
}

// ─── BPM sync ────────────────────────────────────────────────────────────────

/// Seconds per cycle for the subdivision at `index`, given `tempo_bpm`.
/// Mirrors VXN1's `vxn_app::sync::synced_seconds`.
#[inline]
pub fn synced_seconds(tempo_bpm: f32, index: usize) -> f32 {
    let beats = SUBDIVISIONS[index.min(SUBDIVISIONS.len() - 1)].beats;
    beats / (tempo_bpm / 60.0)
}

// ─── Params + delay ──────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug)]
pub struct StereoDelayParams {
    pub on: bool,
    /// Free delay time in ms (used when `sync = false`).
    pub time_ms: f32,
    pub sync: bool,
    /// Index into [`SUBDIVISIONS`] when `sync = true`.
    pub sync_index: usize,
    /// 0.0 ..= [`MAX_FEEDBACK`] (clamped).
    pub feedback: f32,
    /// 0.0 ..= 1.0 (clamped). Linear `(1-mix)·dry + mix·wet`.
    pub mix: f32,
    pub pingpong: bool,
}

impl Default for StereoDelayParams {
    fn default() -> Self {
        Self {
            on: true,
            time_ms: 375.0,
            sync: true,
            sync_index: 10, // "1/8."  (dotted eighth) — matches PARAMETERS.md "3/8" default
            feedback: 0.45,
            mix: 0.25,
            pingpong: false,
        }
    }
}

/// Stereo delay with feedback, ping-pong, BPM sync, and ~100 ms time
/// smoothing.
pub struct StereoDelay {
    buf_l: Ring,
    buf_r: Ring,
    dc_l: DcBlock,
    dc_r: DcBlock,
    samples: Smoothed,
    sr: f32,
    /// Highest legal read offset (capacity - 4 to leave cubic guard taps).
    max_offset: f32,
    feedback: f32,
    /// Smoothed dry/wet, ticked per sample (kills zipper; fades in on switch-on).
    mix: Smoothed,
    /// First `set_params` snaps `mix` to target (no startup sweep from the seed).
    mix_primed: bool,
    pingpong: bool,
    on: bool,
}

impl StereoDelay {
    pub fn new(sample_rate: f32) -> Self {
        let min_samples = (MAX_DELAY_S * sample_rate).ceil() as usize;
        let buf_l = Ring::new(min_samples);
        let buf_r = Ring::new(min_samples);
        let max_offset = (buf_l.capacity() as f32 - 4.0).max(1.0);

        let p = StereoDelayParams::default();
        let init_secs = if p.sync {
            // Default to a moderate tempo so initial samples are well-defined.
            synced_seconds(120.0, p.sync_index)
        } else {
            p.time_ms * 0.001
        };
        let init_samples = (init_secs * sample_rate).clamp(1.0, max_offset);
        let mut samples = Smoothed::new(init_samples, SMOOTH_MS, sample_rate);
        samples.snap(init_samples);

        Self {
            buf_l,
            buf_r,
            dc_l: DcBlock::new(sample_rate, DC_FC_HZ),
            dc_r: DcBlock::new(sample_rate, DC_FC_HZ),
            samples,
            sr: sample_rate,
            max_offset,
            feedback: p.feedback.clamp(0.0, MAX_FEEDBACK),
            mix: Smoothed::new(p.mix.clamp(0.0, 1.0), MIX_SMOOTH_MS, sample_rate),
            mix_primed: false,
            pingpong: p.pingpong,
            on: p.on,
        }
    }

    /// Push new parameter values for the next control block. Updates the
    /// smoothed delay-time target; the smoother glides per-sample inside
    /// [`process`](Self::process).
    pub fn set_params(&mut self, p: &StereoDelayParams, tempo_bpm: f32) {
        self.on = p.on;
        self.feedback = p.feedback.clamp(0.0, MAX_FEEDBACK);
        // Target the param mix while on, 0 while off — the smoother fades the
        // wet both directions across the on/off edge (no click), and `process`
        // only reverts to a bit-exact passthrough once the fade-out hits 0. The
        // first call snaps (no startup fade on a patch loaded with the delay
        // already set).
        let target = if self.on { p.mix.clamp(0.0, 1.0) } else { 0.0 };
        if self.mix_primed {
            self.mix.set_target(target);
        } else {
            self.mix.snap(target);
            self.mix_primed = true;
        }
        self.pingpong = p.pingpong;

        let secs = if p.sync {
            synced_seconds(tempo_bpm, p.sync_index)
        } else {
            p.time_ms.clamp(MIN_DELAY_MS, MAX_DELAY_MS) * 0.001
        };
        let target = (secs * self.sr).clamp(1.0, self.max_offset);
        self.samples.set_target(target);
    }

    /// Process one stereo sample. When `on = false` the wet first fades to 0,
    /// after which this returns `(in_l, in_r)` bit-identical and does no buffer
    /// work — the steady off bus is unchanged, but switch-off doesn't click.
    #[inline]
    pub fn process(&mut self, in_l: f32, in_r: f32) -> (f32, f32) {
        // Bit-exact passthrough only once a switch-off fade has fully reached 0;
        // while the wet ramps down we keep processing so it glides out cleanly.
        if !self.on && self.mix.current() == 0.0 {
            return (in_l, in_r);
        }

        let d = self.samples.tick();
        let tap_l = self.buf_l.read_cubic(d);
        let tap_r = self.buf_r.read_cubic(d);

        let fb_l = self.dc_l.process(tap_l) * self.feedback;
        let fb_r = self.dc_r.process(tap_r) * self.feedback;

        if self.pingpong {
            self.buf_l.push(in_r + fb_r);
            self.buf_r.push(in_l + fb_l);
        } else {
            self.buf_l.push(in_l + fb_l);
            self.buf_r.push(in_r + fb_r);
        }

        // Equal-power crossfade: the delayed wet is decorrelated from dry, so
        // sqrt gains hold total power constant across the sweep (linear gains
        // dip ~3 dB at mix=0.5).
        let mix = self.mix.tick();
        let dry = (1.0 - mix).sqrt();
        let wet = mix.sqrt();
        let out_l = dry * in_l + wet * tap_l;
        let out_r = dry * in_r + wet * tap_r;
        (out_l, out_r)
    }

    /// Zero buffers + DC blocker state. Smoother target is preserved.
    pub fn reset(&mut self) {
        self.buf_l.clear();
        self.buf_r.clear();
        self.dc_l.reset();
        self.dc_r.reset();
    }

    pub fn buffer_capacity(&self) -> usize {
        self.buf_l.capacity()
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn make() -> StereoDelay {
        StereoDelay::new(SR)
    }

    #[test]
    fn buffer_holds_max_delay_at_sr() {
        // 384k samples at 96 kHz × 4 s; next power of two is 524288.
        let d = StereoDelay::new(96_000.0);
        assert!(d.buffer_capacity() as f32 >= MAX_DELAY_S * 96_000.0);
        assert!(d.buffer_capacity().is_power_of_two());
    }

    #[test]
    fn bypass_passes_input_bit_identical() {
        let mut d = make();
        let p = StereoDelayParams {
            on: false,
            ..Default::default()
        };
        d.set_params(&p, 120.0);
        for n in 0..1024 {
            let l = (n as f32 * 0.001).sin();
            let r = (n as f32 * 0.0017).cos();
            let (ol, or_) = d.process(l, r);
            assert_eq!(ol, l, "L not bit-identical at n={n}");
            assert_eq!(or_, r, "R not bit-identical at n={n}");
        }
    }

    #[test]
    fn delay_appears_after_configured_time() {
        let mut d = make();
        let p = StereoDelayParams {
            on: true,
            time_ms: 10.0,
            sync: false,
            feedback: 0.0,
            mix: 1.0,
            pingpong: false,
            ..Default::default()
        };
        d.set_params(&p, 120.0);
        // Settle smoother far past the 100 ms glide (default sync_index is
        // a dotted eighth at 120 BPM, ~375 ms — needs real time to converge).
        for _ in 0..(SR as usize) {
            let _ = d.process(0.0, 0.0);
        }
        let period = (10.0e-3 * SR) as usize;

        // Single-sample impulse on L only.
        let mut peak_l = 0.0_f32;
        let mut at = 0_usize;
        let (_l, _r) = d.process(1.0, 0.0);
        for n in 1..(period * 2) {
            let (l, _r) = d.process(0.0, 0.0);
            if l.abs() > peak_l {
                peak_l = l.abs();
                at = n;
            }
        }
        assert!(peak_l > 0.5, "impulse should reappear, got peak={peak_l}");
        let drift = (at as i64 - period as i64).abs();
        assert!(drift < 4, "peak at {at}, expected ~{period}");
    }

    #[test]
    fn pingpong_routes_l_input_to_r_output() {
        let mut d = make();
        let p = StereoDelayParams {
            on: true,
            time_ms: 5.0,
            sync: false,
            feedback: 0.0,
            mix: 1.0,
            pingpong: true,
            ..Default::default()
        };
        d.set_params(&p, 120.0);
        // Settle smoother past full glide.
        for _ in 0..(SR as usize) {
            let _ = d.process(0.0, 0.0);
        }
        let period = (5.0e-3 * SR) as usize;

        let _ = d.process(1.0, 0.0);
        let mut peak_r = 0.0_f32;
        let mut peak_l = 0.0_f32;
        for _ in 1..(period * 2) {
            let (l, r) = d.process(0.0, 0.0);
            peak_l = peak_l.max(l.abs());
            peak_r = peak_r.max(r.abs());
        }
        assert!(peak_r > 0.5, "L input should emerge on R, got R peak={peak_r}");
        assert!(
            peak_l < 0.05,
            "no L should appear from L input in ping-pong (got {peak_l})"
        );
    }

    #[test]
    fn feedback_caps_at_max() {
        let mut d = make();
        let p = StereoDelayParams {
            on: true,
            time_ms: 5.0,
            sync: false,
            feedback: 5.0, // way over the cap
            mix: 1.0,
            pingpong: false,
            ..Default::default()
        };
        d.set_params(&p, 120.0);
        // Settle.
        for _ in 0..(SR as usize / 5) {
            let _ = d.process(0.0, 0.0);
        }
        // Hit it with a unit impulse then run for a few seconds.
        let _ = d.process(1.0, 0.0);
        let mut peak = 1.0_f32;
        for _ in 0..(SR as usize * 2) {
            let (l, _r) = d.process(0.0, 0.0);
            peak = peak.max(l.abs());
        }
        // With feedback clamped at 0.95, energy decays; without the clamp
        // a feedback of 5.0 would blow up to infinities/NaNs almost instantly.
        assert!(peak.is_finite(), "feedback exploded");
        assert!(peak < 10.0, "feedback should be bounded, got {peak}");
    }

    #[test]
    fn dc_blocker_kills_dc_in_feedback_loop() {
        // Constant DC input — with a DC blocker on the feedback path, the
        // wet sum stays bounded. Without one, every loop trip adds a DC
        // contribution and the output grows linearly.
        let mut d = make();
        let p = StereoDelayParams {
            on: true,
            time_ms: 5.0,
            sync: false,
            feedback: 0.9,
            mix: 1.0,
            pingpong: false,
            ..Default::default()
        };
        d.set_params(&p, 120.0);
        for _ in 0..(SR as usize * 2) {
            let _ = d.process(0.3, 0.0);
        }
        // After 2 s the wet path is well past steady state. DC must be
        // attenuated — wet L should not have run away beyond a modest bound.
        let (l, _r) = d.process(0.3, 0.0);
        assert!(l.abs() < 1.5, "DC leaked into feedback, out={l}");
    }

    #[test]
    fn mix_zero_is_dry() {
        let mut d = make();
        let p = StereoDelayParams {
            on: true,
            mix: 0.0,
            feedback: 0.0,
            sync: false,
            time_ms: 5.0,
            ..Default::default()
        };
        d.set_params(&p, 120.0);
        let (l, r) = d.process(0.42, -0.17);
        assert!((l - 0.42).abs() < 1e-6);
        assert!((r + 0.17).abs() < 1e-6);
    }

    #[test]
    fn mix_half_is_equal_gain() {
        // Equal-power crossfade: with an empty buffer the wet tap is ~0, so
        // out = √(1-mix) * dry = √0.5 ≈ 0.7071 at mix=0.5.
        let mut d = make();
        let p = StereoDelayParams {
            on: true,
            mix: 0.5,
            feedback: 0.0,
            sync: false,
            time_ms: 100.0,
            ..Default::default()
        };
        d.set_params(&p, 120.0);
        let (l, r) = d.process(1.0, 1.0);
        let g = 0.5_f32.sqrt();
        assert!((l - g).abs() < 1e-6, "L gain at mix=0.5: {l}");
        assert!((r - g).abs() < 1e-6, "R gain at mix=0.5: {r}");
    }

    #[test]
    fn synced_seconds_matches_beat_math() {
        // 1/4 at 120 BPM = 0.5 s.
        let q = SUBDIVISIONS.iter().position(|s| s.label == "1/4").unwrap();
        assert!((synced_seconds(120.0, q) - 0.5).abs() < 1e-5);
        assert!((synced_seconds(60.0, q) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn dc_blocker_actually_blocks_dc() {
        let mut dc = DcBlock::new(48_000.0, 10.0);
        let mut last = 0.0;
        for _ in 0..48_000 {
            last = dc.process(1.0);
        }
        // After 1 s of constant 1.0 input the highpass output → ~0.
        assert!(last.abs() < 0.02, "DC blocker leaked: {last}");
    }
}
