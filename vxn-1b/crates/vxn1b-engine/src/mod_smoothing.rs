//! Per-lane modulation-motion smoothing (ticket 0208) — the discontinuity
//! guards VXN2 has and VXN1b's raw per-control-block matrix apply lacks.
//!
//! The matrix ([`crate::eval`]) resolves its dest totals once per control block
//! and the [`crate::bank`] render holds them constant across the block (Amp
//! aside — that's per-frame). A stepped source (square/pulse LFO, note-random,
//! a fast env) routed into a continuous dest therefore lands a hard value step
//! at every control-block edge (~1.5 kHz at 48 kHz), and that step is a click.
//!
//! Two tiers, matched to how click-sensitive each dest is — the same split VXN2
//! settled on ([[vxn2-level-mod-pipeline]]):
//!
//! * **Pitch + XModSweep** — a **cascaded two-pole** per lane, ticked per
//!   [`PITCH_QUANTUM`] samples *inside* the render loop so the audio sees a
//!   sloped ramp, not a block-held stair. The cascade is load-bearing: a single
//!   one-pole is C0 but C1-broken — at a saw/pulse step the output *value* is
//!   continuous but its *velocity* jumps 0→max instantly, and that velocity
//!   step is the click. A second pole makes the output slope start at zero, so
//!   sharp LFO shapes routed to pitch ramp in clean (VXN2's `PitchSmoother`
//!   rationale, ported and trimmed from 16 stack-lanes to VXN1b's 8).
//! * **Non-env Amp** — a single **per-frame one-pole** on the static
//!   (non-envelope) part of the VCA coefficient. Amplitude is the most
//!   click-prone target of all: a residual *block-held* stairstep on a slow
//!   carrier is itself audible, so this glides every sample (like VXN2's
//!   per-sample level ramp), not once per block. The envelope part of the VCA
//!   stays per-frame exact — only the non-env part (an LFO→Amp, velocity→Amp, …
//!   route) is smoothed.
//! * **PWM** — a single **per-quantum one-pole** on the pulse-width offset.
//!   Less click-critical than amplitude but still stepped by a fast source; a
//!   [`PITCH_QUANTUM`]-rate glide inside the render loop keeps the duty cycle
//!   from stair-stepping at block edges.
//!
//! Cutoff/Resonance are deliberately **not** smoothed here: the OTA ladder ramps
//! its own coefficients per frame ([`crate::bank`] `prepare_ramp`/`tick_coeffs`),
//! which already absorbs their block-edge steps.
//!
//! All state is fixed-size `[f32; N]` per smoothed quantity — allocation-free,
//! `Copy`, NEON-friendly. A fresh note **snaps** its lane (both cascade stages
//! and the one-poles) to the block target so the voice starts settled: static
//! sources (velocity, key) land zipper-free and there's no glide from the stolen
//! voice's stale state.

use vxn_dsp::one_pole_coeff;

/// Lanes per bank — the shared DSP kernel width (kept in sync with [`crate::bank`]).
const N: usize = vxn_dsp::CHANNELS_PER_LAYER;

/// Pitch-family cascade advances one step every this many base-rate samples
/// inside the render loop (VXN2's `PITCH_SMOOTH_QUANTUM`). 16 keeps the ramp
/// visibly sub-block (control block is 32) while amortising the pitch recook.
pub const PITCH_QUANTUM: usize = 16;

/// One-pole glide time for PWM / non-env Amp (ms).
const SLOW_MS: f32 = 5.0;

/// Below this a smoother is treated as settled (target and state both within
/// eps), so a patch with no active route on that dest pays no per-quantum
/// recook. In the dest's native unit — well under audible, for pitch (semitones)
/// or PWM (pulse-width fraction) alike.
const SETTLE_EPS: f32 = 1.0e-4;

/// The two pitch-family dests smoothed by the cascade, in this fixed order.
/// Index 0 = `Pitch` (both oscs), 1 = `XModSweep` (mode-gated osc).
pub const PITCH: usize = 0;
pub const SWEEP: usize = 1;
const N_PITCH: usize = 2;

/// Per-lane motion smoothers for one render bank.
#[derive(Clone, Copy, Debug)]
pub struct MotionSmoother {
    /// Cascade stage 1 (intermediate) for the pitch-family dests: `[dest][lane]`.
    p_stage1: [[f32; N]; N_PITCH],
    /// Cascade stage 2 (= smoothed output) for the pitch-family dests.
    p_state: [[f32; N]; N_PITCH],
    /// Block-rate one-pole state for the PWM dest, per lane.
    pwm: [f32; N],
    /// Block-rate one-pole state for the non-env Amp coefficient, per lane.
    amp_stat: [f32; N],
    /// Cascade coeff, calibrated at the *quantum* tick rate (`sr / PITCH_QUANTUM`).
    pitch_coeff: f32,
    /// Amp one-pole coeff, calibrated at the *per-frame* (base-sample) rate.
    amp_coeff: f32,
    /// PWM one-pole coeff, calibrated at the *quantum* tick rate.
    pwm_coeff: f32,
}

impl MotionSmoother {
    /// `sample_rate` is the base (non-oversampled) rate the render loop runs at.
    pub fn new(sample_rate: f32) -> Self {
        // Cascade time constant ≈ one control block, but ticked per quantum, so
        // calibrate the coeff at the quantum rate (matches VXN2).
        let block_ms = vxn_dsp::CONTROL_BLOCK as f32 / sample_rate * 1000.0;
        let pitch_coeff = one_pole_coeff(block_ms, sample_rate / PITCH_QUANTUM as f32);
        // Amp glides every sample; PWM every quantum.
        let amp_coeff = one_pole_coeff(SLOW_MS, sample_rate);
        let pwm_coeff = one_pole_coeff(SLOW_MS, sample_rate / PITCH_QUANTUM as f32);
        Self {
            p_stage1: [[0.0; N]; N_PITCH],
            p_state: [[0.0; N]; N_PITCH],
            pwm: [0.0; N],
            amp_stat: [0.0; N],
            pitch_coeff,
            amp_coeff,
            pwm_coeff,
        }
    }

    /// Re-cook coeffs for a new sample rate; state is cleared to zero (callers
    /// reset the bank around a sample-rate change).
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        *self = Self::new(sample_rate);
    }

    /// Zero all state (bank reset).
    pub fn reset(&mut self) {
        self.p_stage1 = [[0.0; N]; N_PITCH];
        self.p_state = [[0.0; N]; N_PITCH];
        self.pwm = [0.0; N];
        self.amp_stat = [0.0; N];
    }

    /// Snap one lane's pitch cascade (both stages) to the block targets, so a
    /// fresh note starts settled rather than gliding up from the previous voice.
    #[inline]
    pub fn snap_pitch(&mut self, v: usize, pitch_target: f32, sweep_target: f32) {
        self.p_stage1[PITCH][v] = pitch_target;
        self.p_state[PITCH][v] = pitch_target;
        self.p_stage1[SWEEP][v] = sweep_target;
        self.p_state[SWEEP][v] = sweep_target;
    }

    /// Snap one lane's PWM / Amp-stat one-poles to their block targets.
    #[inline]
    pub fn snap_slow(&mut self, v: usize, pwm_target: f32, amp_stat_target: f32) {
        self.pwm[v] = pwm_target;
        self.amp_stat[v] = amp_stat_target;
    }

    /// Advance one lane's pitch cascade one quantum step toward the targets and
    /// return the smoothed `(pitch, sweep)` offsets. `stage1` chases the target;
    /// `state` (the output) chases `stage1` — the second stage is what gives a
    /// zero starting slope so sharp LFO-into-pitch steps ramp in without a click.
    #[inline]
    pub fn tick_pitch(&mut self, v: usize, pitch_target: f32, sweep_target: f32) -> (f32, f32) {
        let a = self.pitch_coeff;
        self.p_stage1[PITCH][v] += a * (pitch_target - self.p_stage1[PITCH][v]);
        self.p_state[PITCH][v] += a * (self.p_stage1[PITCH][v] - self.p_state[PITCH][v]);
        self.p_stage1[SWEEP][v] += a * (sweep_target - self.p_stage1[SWEEP][v]);
        self.p_state[SWEEP][v] += a * (self.p_stage1[SWEEP][v] - self.p_state[SWEEP][v]);
        (self.p_state[PITCH][v], self.p_state[SWEEP][v])
    }

    /// Whether lane `v`'s pitch cascade needs ticking this block: any nonzero
    /// target, or residual energy still gliding back toward zero after a route
    /// turned off. When false the render loop keeps the block-start `inc` and
    /// skips the per-quantum pitch recook.
    #[inline]
    pub fn pitch_active(&self, v: usize, pitch_target: f32, sweep_target: f32) -> bool {
        pitch_target.abs() > SETTLE_EPS
            || sweep_target.abs() > SETTLE_EPS
            || self.p_state[PITCH][v].abs() > SETTLE_EPS
            || self.p_state[SWEEP][v].abs() > SETTLE_EPS
            || self.p_stage1[PITCH][v].abs() > SETTLE_EPS
            || self.p_stage1[SWEEP][v].abs() > SETTLE_EPS
    }

    /// Whether lane `v`'s PWM one-pole needs ticking (nonzero target or residual
    /// state); when false the render loop keeps the block-start pulse width.
    #[inline]
    pub fn pwm_active(&self, v: usize, target: f32) -> bool {
        target.abs() > SETTLE_EPS || self.pwm[v].abs() > SETTLE_EPS
    }

    /// Advance lane `v`'s PWM one-pole one quantum step and return the smoothed
    /// PWM offset.
    #[inline]
    pub fn tick_pwm(&mut self, v: usize, target: f32) -> f32 {
        self.pwm[v] += self.pwm_coeff * (target - self.pwm[v]);
        self.pwm[v]
    }

    /// Lane `v`'s current smoothed PWM offset, without advancing (block-start peek).
    #[inline]
    pub fn pwm_current(&self, v: usize) -> f32 {
        self.pwm[v]
    }

    /// Advance lane `v`'s non-env Amp one-pole one **frame** step and return the
    /// smoothed static Amp coefficient. Ticked per sample (not per block) because
    /// a block-held amplitude stair is itself an audible click on a slow carrier.
    #[inline]
    pub fn tick_amp_stat(&mut self, v: usize, target: f32) -> f32 {
        self.amp_stat[v] += self.amp_coeff * (target - self.amp_stat[v]);
        self.amp_stat[v]
    }

    /// Lane `v`'s current smoothed Amp coefficient, without advancing.
    #[inline]
    pub fn amp_stat_current(&self, v: usize) -> f32 {
        self.amp_stat[v]
    }

    /// True when lane `v`'s Amp one-pole is within [`SETTLE_EPS`] of `target` —
    /// the render loop keeps its envelope-static constant-amp fast path while a
    /// non-env Amp route is still gliding.
    #[inline]
    pub fn amp_stat_settled(&self, v: usize, target: f32) -> bool {
        (self.amp_stat[v] - target).abs() <= SETTLE_EPS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    #[test]
    fn cascade_output_slope_starts_at_zero() {
        // Step the target from 0 to 1; the cascade's first step must be far
        // smaller than a single one-pole's would be (coeff·1), because the
        // second stage chases a still-near-zero stage1 — that near-zero starting
        // slope is what kills the pitch click.
        let mut s = MotionSmoother::new(SR);
        let (p0, _) = s.tick_pitch(0, 1.0, 0.0);
        // Single-pole first step would be `pitch_coeff` (~0.39). The cascade
        // output must be much less — the product of the two stage responses.
        assert!(p0 < 0.2, "cascade first step {p0} should be well below a lone pole's");
        assert!(p0 > 0.0, "but it must start moving");
    }

    #[test]
    fn cascade_converges_to_target() {
        let mut s = MotionSmoother::new(SR);
        let mut last = 0.0;
        for _ in 0..64 {
            let (p, _) = s.tick_pitch(0, 3.0, 0.0);
            last = p;
        }
        assert!((last - 3.0).abs() < 1e-2, "cascade should reach the target, got {last}");
    }

    #[test]
    fn snap_lands_settled_no_glide() {
        let mut s = MotionSmoother::new(SR);
        s.snap_pitch(0, 5.0, -2.0);
        // A tick right after snap must not move (already at target).
        let (p, sw) = s.tick_pitch(0, 5.0, -2.0);
        assert!((p - 5.0).abs() < 1e-6 && (sw + 2.0).abs() < 1e-6);
    }

    #[test]
    fn pitch_active_false_when_settled_at_zero() {
        let s = MotionSmoother::new(SR);
        assert!(!s.pitch_active(0, 0.0, 0.0), "no route, zero state → inactive");
        assert!(s.pitch_active(0, 0.5, 0.0), "nonzero target → active");
    }

    #[test]
    fn pitch_active_true_while_gliding_back_to_zero() {
        // Route was on, now off (target 0) but state still has energy → must stay
        // active so it glides down instead of snapping to 0 (which would click).
        let mut s = MotionSmoother::new(SR);
        for _ in 0..4 {
            s.tick_pitch(0, 4.0, 0.0);
        }
        assert!(s.pitch_active(0, 0.0, 0.0), "residual energy keeps it active");
    }

    #[test]
    fn pwm_one_pole_glides_and_settles() {
        let mut s = MotionSmoother::new(SR);
        let first = s.tick_pwm(0, 1.0);
        assert!(first > 0.0 && first < 0.5, "per-quantum step is partial, got {first}");
        for _ in 0..256 {
            s.tick_pwm(0, 1.0);
        }
        assert!((s.tick_pwm(0, 1.0) - 1.0).abs() < 1e-2);
    }

    #[test]
    fn amp_stat_per_frame_glide_and_settle_query() {
        let mut s = MotionSmoother::new(SR);
        assert!(!s.amp_stat_settled(0, 1.0), "starts at 0, target 1 → unsettled");
        for _ in 0..4096 {
            s.tick_amp_stat(0, 1.0);
        }
        assert!(s.amp_stat_settled(0, 1.0), "should settle at the target");
        assert!((s.amp_stat_current(0) - 1.0).abs() < 1e-3);
    }
}
