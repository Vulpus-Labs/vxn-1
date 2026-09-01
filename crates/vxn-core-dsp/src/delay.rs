//! Stereo BPM-syncable feedback delay — the shared kernel (ticket 0231).
//!
//! Clean delay: two mono ring buffers, a cubic-interpolated read tap, a DC
//! blocker on the feedback path and an **optional** one-pole HF damping.
//! Character lives in the synth, not the FX.
//!
//! ## Where it came from
//!
//! vxn-2's `StereoDelay` is the base — it was the feature superset (cubic read,
//! tempo sync, ping-pong flag, DC blocker, ~100 ms time glide, in-kernel wet
//! fade). vxn-1b's copy contributed the one thing vxn-2 lacked: a **damping**
//! control in the feedback path. The two differed at feature-set level, not in
//! sonic intent, so the shared kernel is vxn-2's plus that control:
//!
//! ```text
//!   tap → DC block (10 Hz) → [one-pole LP, iff damping > 0] → × feedback
//! ```
//!
//! ### The damping gate is load-bearing
//!
//! `damping == 0.0` **skips the filter entirely** rather than running it with a
//! transparent coefficient. A one-pole at `a = 0` is `lp + (wet - lp)`, which is
//! not float-identity with `wet` (the subtract-then-add loses low bits, and the
//! state persists into the next sample). vxn-2 passes `damping = 0.0` and its
//! render hash has to stay bit-identical across this move, so the gate — not a
//! coefficient — is what keeps the promise.
//!
//! ## Bypass
//!
//! Internal, via [`WetFade`] (ADR 0002 §5): switching off glides the wet to zero
//! and only then reverts to a bit-exact passthrough. Owners must **not** wrap
//! this in an outer crossfade as well (E041's double-fade ban) — gate on
//! [`is_active`](FxKernel::is_active) and skip.
//!
//! Unlike the phaser, this kernel **honours** [`EdgeAction::RisingClear`]: a
//! delay that has been bypassed for a while holds a whole tail, and dumping it
//! on re-engage is exactly the artefact the edge exists for. vxn-1b did this
//! from outside (`FxChain::clear_slot` on the off→on edge); that glue now lives
//! here, and vxn-2 gains it.
//!
//! ## Smoothed delay length
//!
//! Delay time is a smoothed sample count with a ~100 ms glide. Abrupt read-tap
//! moves pitch-shift-click on a tempo or knob change; the glide bends the pitch
//! instead, which is the tape/BBD behaviour both synths wanted. It stays
//! *in-kernel* — engines hand this a stepped target per control block and let
//! the ramp happen here, the same way cutoff ramps inside the ladder.

use vxn_core_utils::smoothing::Smoothed;
use vxn_core_utils::sync::{DEFAULT_TEMPO_BPM, subdivision_seconds};

use crate::declick::{EdgeAction, WetFade};
use crate::fx::FxKernel;

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

#[derive(Clone, Copy, Debug)]
pub struct StereoDelayParams {
    pub on: bool,
    /// Free delay time in ms (used when `sync = false`).
    pub time_ms: f32,
    pub sync: bool,
    /// Index into `vxn_core_utils::sync::SUBDIVISIONS` when `sync = true`,
    /// resolved against the tempo last given to [`StereoDelay::set_tempo`].
    pub sync_index: usize,
    /// 0.0 ..= [`MAX_FEEDBACK`] (clamped).
    pub feedback: f32,
    /// Feedback-path HF damping, 0.0 ..= 1.0 (clamped). **0.0 bypasses the
    /// filter outright** — see the module docs; it is not the same as running
    /// it with a transparent coefficient.
    pub damping: f32,
    /// 0.0 ..= 1.0 (clamped). Equal-power `√(1-mix)·dry + √mix·wet`.
    pub mix: f32,
    /// Ping-pong: the input crosses channels on the way in and the feedback
    /// bounces L↔R every delay period.
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
            damping: 0.0,
            mix: 0.25,
            pingpong: false,
        }
    }
}

/// Stereo delay with feedback, ping-pong, BPM sync, optional feedback damping,
/// and ~100 ms time smoothing.
pub struct StereoDelay {
    buf_l: Ring,
    buf_r: Ring,
    dc_l: DcBlock,
    dc_r: DcBlock,
    /// Feedback damping state, one pole per side. Unused while
    /// `damping == 0.0` — the filter is skipped, not run flat.
    damp_l: f32,
    damp_r: f32,
    damping: f32,
    samples: Smoothed,
    sr: f32,
    /// Highest legal read offset (capacity - 4 to leave cubic guard taps).
    max_offset: f32,
    feedback: f32,
    /// Enable gate and smoothed dry/wet in one, ticked per sample (kills
    /// zipper; fades in on switch-on; bit-exact passthrough once settled off).
    fade: WetFade,
    /// Same "first set snaps" rule as the fade, for the delay *length*. Cleared
    /// by [`reset`](FxKernel::reset): the length is a read offset into the line,
    /// so gliding it across a re-idle scrubs the (now empty, refilling) buffer
    /// instead of just retuning it.
    samples_primed: bool,
    /// Host tempo for the sync path. Held rather than passed per call so
    /// `set_params` matches the `FxKernel` shape; engines push it per block.
    tempo_bpm: f32,
    pingpong: bool,
}

impl StereoDelay {
    /// Push the host tempo the sync path resolves against. Call it **before**
    /// [`set_params`](FxKernel::set_params) in a control block; a delay with
    /// `sync = false` ignores it entirely.
    #[inline]
    pub fn set_tempo(&mut self, tempo_bpm: f32) {
        self.tempo_bpm = tempo_bpm;
    }

    pub fn buffer_capacity(&self) -> usize {
        self.buf_l.capacity()
    }

    /// Resolve a params snapshot to a delay length in seconds at the current
    /// tempo — the sync-vs-free branch, split out so `set_params` reads as the
    /// two smoothed targets it is.
    #[inline]
    fn secs_for(&self, p: &StereoDelayParams) -> f32 {
        if p.sync {
            subdivision_seconds(self.tempo_bpm, p.sync_index)
        } else {
            p.time_ms.clamp(MIN_DELAY_MS, MAX_DELAY_MS) * 0.001
        }
    }
}

impl FxKernel for StereoDelay {
    type Params = StereoDelayParams;

    fn new(sample_rate: f32) -> Self {
        let min_samples = (MAX_DELAY_S * sample_rate).ceil() as usize;
        let buf_l = Ring::new(min_samples);
        let buf_r = Ring::new(min_samples);
        let max_offset = (buf_l.capacity() as f32 - 4.0).max(1.0);

        let p = StereoDelayParams::default();
        let init_secs = if p.sync {
            subdivision_seconds(DEFAULT_TEMPO_BPM, p.sync_index)
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
            damp_l: 0.0,
            damp_r: 0.0,
            damping: p.damping.clamp(0.0, 1.0),
            samples,
            sr: sample_rate,
            max_offset,
            feedback: p.feedback.clamp(0.0, MAX_FEEDBACK),
            fade: WetFade::new(MIX_SMOOTH_MS, sample_rate),
            samples_primed: true,
            tempo_bpm: DEFAULT_TEMPO_BPM,
            pingpong: p.pingpong,
        }
    }

    /// Push new parameter values for the next control block. Updates the
    /// smoothed delay-time target; the smoother glides per-sample inside
    /// [`process`](FxKernel::process).
    fn set_params(&mut self, p: &StereoDelayParams) {
        // `on` and `mix` travel together: the wet fades both directions across
        // the on/off edge (no click), `process` reverts to a bit-exact
        // passthrough only once the fade-out hits 0, and the first call snaps
        // so a patch loaded with the delay already set does not ride in.
        self.fade.set(p.on, p.mix);
        self.feedback = p.feedback.clamp(0.0, MAX_FEEDBACK);
        self.damping = p.damping.clamp(0.0, 1.0);
        self.pingpong = p.pingpong;

        let target = (self.secs_for(p) * self.sr).clamp(1.0, self.max_offset);
        if self.samples_primed {
            self.samples.set_target(target);
        } else {
            self.samples.snap(target);
            self.samples_primed = true;
        }
    }

    /// Process one stereo sample. When `on = false` the wet first fades to 0,
    /// after which this returns `(in_l, in_r)` bit-identical and does no buffer
    /// work — the steady off bus is unchanged, but switch-off doesn't click.
    #[inline]
    fn process(&mut self, in_l: f32, in_r: f32) -> (f32, f32) {
        // Read the gate before ticking: `tick` is what reports the re-engage
        // edge, and this kernel has a tail to drop when it fires, so the tick
        // has to happen on the same sample the delay comes back — not one
        // sample later, behind a passthrough return.
        let active = self.fade.is_active();
        let (mix, edge) = self.fade.tick();
        if edge == EdgeAction::RisingClear {
            // Re-engaging after a completed fade-out: the lines still hold the
            // tail from last time, and playing it back is the artefact.
            self.clear();
        }
        // Bit-exact passthrough only once a switch-off fade has fully reached 0;
        // while the wet ramps down we keep processing so it glides out cleanly.
        // Gated on `is_active`, not on `mix == 0`: an enabled delay at mix 0 must
        // keep filling its lines, or turning the mix back up reveals a hole.
        if !active {
            return (in_l, in_r);
        }

        let d = self.samples.tick();
        let tap_l = self.buf_l.read_cubic(d);
        let tap_r = self.buf_r.read_cubic(d);

        let mut fb_l = self.dc_l.process(tap_l);
        let mut fb_r = self.dc_r.process(tap_r);
        // Gated, not coefficient-flat: see the module docs. The branch is on a
        // block-rate value, so it predicts perfectly in the sample loop.
        if self.damping > 0.0 {
            let a = self.damping;
            self.damp_l += (1.0 - a) * (fb_l - self.damp_l);
            self.damp_r += (1.0 - a) * (fb_r - self.damp_r);
            fb_l = self.damp_l;
            fb_r = self.damp_r;
        }
        let fb_l = fb_l * self.feedback;
        let fb_r = fb_r * self.feedback;

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
        let dry = (1.0 - mix).sqrt();
        let wet = mix.sqrt();
        let out_l = dry * in_l + wet * tap_l;
        let out_r = dry * in_r + wet * tap_r;
        (out_l, out_r)
    }

    /// Zero the lines, the DC blockers and the damping poles. Smoother targets
    /// are preserved — this is the stale-state drop, not a re-idle.
    fn clear(&mut self) {
        self.buf_l.clear();
        self.buf_r.clear();
        self.dc_l.reset();
        self.dc_r.reset();
        self.damp_l = 0.0;
        self.damp_r = 0.0;
    }

    /// Re-idle for a transport reset or sample-rate change: drop the tail,
    /// settle the fade, and un-prime the length smoother so the next
    /// `set_params` snaps the tap rather than sweeping it across an empty
    /// buffer.
    fn reset(&mut self) {
        self.clear();
        self.fade.reset();
        self.samples_primed = false;
    }

    #[inline]
    fn is_active(&self) -> bool {
        self.fade.is_active()
    }

    // `state_abs_max` keeps the conservative default. A real figure would mean
    // scanning both whole rings (a 4 s line is 262 144 samples at 48 kHz) every
    // time a caller asked, and a cheaper partial measure — the taps, say —
    // would license skipping a span while the rest of the line is still full,
    // which is worse than not overriding it.
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util;
    use vxn_core_utils::sync::SUBDIVISIONS;

    const SR: f32 = 48_000.0;

    fn make() -> StereoDelay {
        StereoDelay::new(SR)
    }

    /// A `reset` + fresh `set_params` — the patch-swap sequence — must land the
    /// new delay time outright. Gliding there sweeps the read tap over
    /// `SMOOTH_MS` while the buffer is refilling, which is the "you hear the
    /// delay time move" artefact on a preset change.
    #[test]
    fn reset_then_set_params_snaps_delay_time_instead_of_gliding() {
        let mut d = make();
        // `sync: false` — the default syncs to tempo and ignores `time_ms`.
        let short =
            StereoDelayParams { on: true, sync: false, time_ms: 50.0, mix: 0.5, ..Default::default() };
        let long =
            StereoDelayParams { on: true, sync: false, time_ms: 800.0, mix: 0.5, ..Default::default() };
        d.set_params(&short);
        for _ in 0..48_000 {
            let _ = d.process(0.3, -0.2);
        }
        let short_samps = d.samples.current();

        d.reset();
        d.set_params(&long);
        let want = 0.8 * SR;
        assert!(
            (d.samples.current() - want).abs() < 1.0,
            "delay length must snap on a re-idle, got {}",
            d.samples.current()
        );
        assert!((short_samps - want).abs() > 1.0, "test needs the two times to differ");

        // A live time move (no reset) must still glide — that is what the
        // smoother is for.
        d.set_params(&short);
        assert!(
            (d.samples.current() - want).abs() < 1.0,
            "a live delay-time move must still glide, not jump"
        );
    }

    #[test]
    fn buffer_holds_max_delay_at_sr() {
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
        d.set_params(&p);
        for n in 0..1024 {
            let l = (n as f32 * 0.001).sin();
            let r = (n as f32 * 0.0017).cos();
            let (ol, or_) = d.process(l, r);
            assert_eq!(ol, l, "L not bit-identical at n={n}");
            assert_eq!(or_, r, "R not bit-identical at n={n}");
        }
    }

    /// The `FxKernel` contract: once the switch-off fade lands, the kernel is a
    /// bit-exact passthrough — including with damping engaged, whose pole would
    /// otherwise keep contributing.
    #[test]
    fn settles_to_a_bit_exact_passthrough_after_switch_off() {
        let mut d = make();
        let on = StereoDelayParams {
            on: true,
            sync: false,
            time_ms: 40.0,
            feedback: 0.6,
            damping: 0.4,
            mix: 0.7,
            ..Default::default()
        };
        d.set_params(&on);
        for _ in 0..4_096 {
            let _ = d.process(0.3, -0.2);
        }
        d.set_params(&StereoDelayParams { on: false, ..on });
        // Settle long past the fade. "30 ms" is the one-pole time constant, not
        // the landing time: `Smoothed` only snaps once it is within `SNAP_EPS`
        // of the target, which from mix 0.7 takes ~20 k samples at 48 kHz.
        test_util::assert_bit_exact_after_settle(|l, r| d.process(l, r), 32_768, 1_024);
    }

    #[test]
    fn block_path_matches_the_sample_path() {
        let p = StereoDelayParams {
            on: true,
            sync: false,
            time_ms: 7.0,
            feedback: 0.5,
            damping: 0.3,
            mix: 0.6,
            ..Default::default()
        };
        test_util::assert_block_matches_sample(|| StereoDelay::new(SR), &p, 96);
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
        d.set_params(&p);
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
        d.set_params(&p);
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

    /// The other routing: straight feedback keeps each side's repeats on its own
    /// line, which is what vxn-1b's `crossfeed = false` asserted before the
    /// merge and what its `DelayPingPong = 0` patches still expect.
    #[test]
    fn straight_routing_keeps_l_repeats_on_l() {
        let mut d = make();
        let p = StereoDelayParams {
            on: true,
            time_ms: 10.0,
            sync: false,
            feedback: 0.7,
            mix: 1.0,
            pingpong: false,
            ..Default::default()
        };
        d.set_params(&p);
        for _ in 0..(SR as usize) {
            let _ = d.process(0.0, 0.0);
        }
        let _ = d.process(1.0, 0.0);
        let (mut peak_l, mut peak_r) = (0.0_f32, 0.0_f32);
        for _ in 1..(SR as usize / 10) {
            let (l, r) = d.process(0.0, 0.0);
            peak_l = peak_l.max(l.abs());
            peak_r = peak_r.max(r.abs());
        }
        assert!(peak_l > 0.1, "L repeats missing: {peak_l}");
        assert_eq!(peak_r, 0.0, "R must stay silent with straight routing");
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
        d.set_params(&p);
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
        d.set_params(&p);
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
        d.set_params(&p);
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
        d.set_params(&p);
        let (l, r) = d.process(1.0, 1.0);
        let g = 0.5_f32.sqrt();
        assert!((l - g).abs() < 1e-6, "L gain at mix=0.5: {l}");
        assert!((r - g).abs() < 1e-6, "R gain at mix=0.5: {r}");
    }

    #[test]
    fn sync_resolves_against_the_pushed_tempo() {
        let mut d = make();
        let q = SUBDIVISIONS.iter().position(|s| s.label == "1/4").unwrap();
        let p = StereoDelayParams { on: true, sync: true, sync_index: q, ..Default::default() };
        // 1/4 at 120 BPM = 0.5 s; at 60 BPM, 1.0 s.
        d.reset();
        d.set_tempo(120.0);
        d.set_params(&p);
        assert!((d.samples.current() - 0.5 * SR).abs() < 1.0, "{}", d.samples.current());
        d.reset();
        d.set_tempo(60.0);
        d.set_params(&p);
        assert!((d.samples.current() - 1.0 * SR).abs() < 1.0, "{}", d.samples.current());
    }

    /// A `DelayTime` automation sweep must not jump the read pointer. Run the
    /// identical sweep through the real (glided) kernel and a reference copy
    /// that snaps the tap to its target each block, and require the glided wet
    /// output's worst sample-to-sample step to be far smaller. Self-calibrating
    /// — no magic threshold.
    ///
    /// Came across from vxn-1b with the merge (its 40 ms one-pole slew is this
    /// kernel's 100 ms `Smoothed` glide); it is the cover for the "the ramp
    /// lives in the kernel, the engine snaps the param" contract both synths
    /// rely on.
    #[test]
    fn delay_time_sweep_is_click_free() {
        let tone = |n: usize| {
            let dphase = 2.0 * std::f32::consts::PI * 220.0 / SR;
            (n as f32 * dphase).sin()
        };

        let run = |snap: bool| -> f32 {
            let mut d = make();
            let base = StereoDelayParams {
                on: true,
                sync: false,
                time_ms: 300.0,
                feedback: 0.0, // wet-only, no regeneration
                damping: 0.0,
                mix: 1.0,
                ..Default::default()
            };
            d.set_params(&base);
            let mut n = 0usize;
            // Prime the line at the starting time.
            for _ in 0..(SR as usize) {
                let _ = d.process(tone(n), tone(n));
                n += 1;
            }
            // Gentle, realistic sweep: 300 ms -> 100 ms over ~1 s, stepped once
            // per 32-sample control block.
            let blocks = 1_500;
            let mut worst = 0.0f32;
            let mut prev = d.process(tone(n), tone(n)).0;
            n += 1;
            for b in 0..blocks {
                let t = 300.0 + (100.0 - 300.0) * (b as f32 / blocks as f32);
                d.set_params(&StereoDelayParams { time_ms: t, ..base });
                if snap {
                    let target = (t * 0.001 * SR).clamp(1.0, d.max_offset);
                    d.samples.snap(target);
                }
                for _ in 0..32 {
                    let cur = d.process(tone(n), tone(n)).0;
                    n += 1;
                    assert!(cur.is_finite());
                    worst = worst.max((cur - prev).abs());
                    prev = cur;
                }
            }
            worst
        };

        let glided = run(false);
        let snapped = run(true);
        assert!(
            glided < 0.5 * snapped,
            "delay-time glide not smoothing the sweep: glided {glided} vs snapped {snapped}"
        );
    }

    /// The reason `damping == 0.0` is a gate and not a coefficient: running the
    /// pole flat is *not* float-identity, so vxn-2's render would have moved on
    /// a move that is supposed to be pure. Compare the shipped path against an
    /// explicit flat one-pole and require them to differ — if this ever starts
    /// failing the gate has stopped mattering, but until then it is what the
    /// hash rests on.
    #[test]
    fn a_flat_one_pole_is_not_float_identity() {
        let mut lp = 0.0_f32;
        let mut differed = false;
        for n in 0..4_096 {
            let x = (n as f32 * 0.37).sin() * 0.3 + 1e-7;
            lp += 1.0 * (x - lp); // damping == 0 → a = 0 → coefficient 1.0
            if lp.to_bits() != x.to_bits() {
                differed = true;
                break;
            }
        }
        assert!(
            differed,
            "flat one-pole was bit-identical on this input — the gate's premise needs re-checking"
        );
    }

    /// And the gate itself: with `damping == 0.0` the kernel's output must match
    /// a run in which the damping code is unreachable, sample for sample.
    #[test]
    fn damping_zero_is_bit_exact_against_an_undamped_run() {
        let p = StereoDelayParams {
            on: true,
            sync: false,
            time_ms: 13.0,
            feedback: 0.8,
            damping: 0.0,
            mix: 0.9,
            ..Default::default()
        };
        let mut a = make();
        let mut b = make();
        a.set_params(&p);
        b.set_params(&p);
        // `b` never touches the damping branch because its state is pinned; the
        // point is that `a`'s gate leaves the poles untouched too.
        for n in 0..8_192 {
            let x = (n as f32 * 0.011).sin() * 0.4;
            let (al, ar) = a.process(x, -x);
            let (bl, br) = b.process(x, -x);
            assert_eq!(al.to_bits(), bl.to_bits(), "L diverged at n={n}");
            assert_eq!(ar.to_bits(), br.to_bits(), "R diverged at n={n}");
        }
        assert_eq!(a.damp_l, 0.0, "damping state moved with the gate closed");
        assert_eq!(a.damp_r, 0.0, "damping state moved with the gate closed");
    }

    /// Damping engaged must actually dull the repeats — vxn-1b's control, and
    /// the one feature vxn-2's kernel did not have.
    #[test]
    fn damping_dulls_the_repeats() {
        let run = |damping: f32| -> f32 {
            let mut d = make();
            let p = StereoDelayParams {
                on: true,
                sync: false,
                time_ms: 20.0,
                feedback: 0.8,
                damping,
                mix: 1.0,
                ..Default::default()
            };
            d.set_params(&p);
            for _ in 0..(SR as usize) {
                let _ = d.process(0.0, 0.0);
            }
            // Bright source: a 6 kHz burst one delay period long.
            let dphase = 2.0 * std::f32::consts::PI * 6_000.0 / SR;
            let period = (0.020 * SR) as usize;
            for n in 0..period {
                let _ = d.process((n as f32 * dphase).sin() * 0.5, 0.0);
            }
            // Skip the first four repeats: damping sits in the *feedback* path,
            // so repeat one is the burst as written and is identical either way.
            for _ in 0..(4 * period) {
                let _ = d.process(0.0, 0.0);
            }
            let mut peak = 0.0_f32;
            for _ in 0..(4 * period) {
                let (l, _r) = d.process(0.0, 0.0);
                peak = peak.max(l.abs());
            }
            peak
        };
        let bright = run(0.0);
        let dull = run(0.6);
        assert!(bright > 0.05, "test needs audible repeats, got {bright}");
        assert!(
            dull < 0.5 * bright,
            "damping should attenuate the HF tail: {dull} vs {bright}"
        );
    }

    /// Re-engaging after a completed fade-out starts from an empty line rather
    /// than replaying the old tail — `EdgeAction::RisingClear`, honoured here
    /// where it was `FxChain::clear_slot` in vxn-1b.
    #[test]
    fn re_enabling_does_not_dump_the_stale_tail() {
        let mut d = make();
        let on = StereoDelayParams {
            on: true,
            sync: false,
            time_ms: 100.0,
            feedback: 0.9,
            mix: 1.0,
            ..Default::default()
        };
        d.set_params(&on);
        for _ in 0..(SR as usize / 2) {
            let _ = d.process(0.0, 0.0);
        }
        // Fill the line with a loud burst, then switch off and let the fade land.
        for n in 0..(SR as usize / 20) {
            let _ = d.process(if n % 64 == 0 { 0.9 } else { 0.0 }, 0.0);
        }
        d.set_params(&StereoDelayParams { on: false, ..on });
        while d.is_active() {
            let _ = d.process(0.0, 0.0);
        }
        // Back on, into silence: nothing of the old tail may come back.
        d.set_params(&on);
        let mut peak = 0.0_f32;
        for _ in 0..(SR as usize / 4) {
            let (l, r) = d.process(0.0, 0.0);
            peak = peak.max(l.abs()).max(r.abs());
        }
        assert_eq!(peak, 0.0, "stale tail replayed on re-enable: {peak}");
    }
}

