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
//! * **PWM** and **cross-mod amount** — a single **per-quantum one-pole** each,
//!   on the pulse-width offset and on the PM index offset (0242). Less
//!   click-critical than amplitude but still stepped by a fast source; a
//!   [`PITCH_QUANTUM`]-rate glide inside the render loop keeps the duty cycle
//!   (and the FM index, which is timbre in the same way) from stair-stepping at
//!   block edges.
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

/// A one-pole smoother held per lane, for one smoothed quantity.
///
/// Every non-pitch dest smoothed here is the same recurrence —
/// `state += coeff · (target − state)` — over a `[f32; N]` lane array. The
/// coefficient is *not* a field: it belongs to the tier (`slow_coeff` per
/// quantum, `amp_coeff` per frame) rather than to the quantity, so
/// [`MotionSmoother`] owns it and passes it in. That keeps this type a plain
/// `Copy` array, and keeps a new smoothed dest to one field instead of a field
/// plus four hand-written methods.
///
/// The pitch cascade is deliberately **not** built on this: it is two poles with
/// its own coefficient and a C1-continuity rationale (see the module docs).
#[derive(Clone, Copy, Debug, Default)]
struct LaneOnePole([f32; N]);

impl LaneOnePole {
    /// Snap one lane straight to `target` — a fresh note starts settled rather
    /// than gliding up from whatever the stolen voice left behind.
    #[inline]
    fn snap(&mut self, v: usize, target: f32) {
        self.0[v] = target;
    }

    /// This lane's current value, without advancing it.
    #[inline]
    fn current(&self, v: usize) -> f32 {
        self.0[v]
    }

    /// Advance one lane a step toward `target` and return the new value.
    #[inline]
    fn tick(&mut self, v: usize, target: f32, coeff: f32) -> f32 {
        self.0[v] += coeff * (target - self.0[v]);
        self.0[v]
    }

    /// Whether this lane is worth ticking: a nonzero target, or residual state
    /// still gliding back toward zero after a route turned off. When false the
    /// render loop keeps its block-start value and skips the per-quantum recook.
    #[inline]
    fn active(&self, v: usize, target: f32) -> bool {
        (target - self.0[v]).abs() > SETTLE_EPS || self.0[v].abs() > SETTLE_EPS
    }

    /// Whether this lane has arrived at `target` — distinct from [`Self::active`],
    /// which also reports a lane displaced from zero with nothing to chase.
    #[inline]
    fn settled(&self, v: usize, target: f32) -> bool {
        (self.0[v] - target).abs() <= SETTLE_EPS
    }
}

/// Per-lane motion smoothers for one render bank.
#[derive(Clone, Copy, Debug)]
pub struct MotionSmoother {
    /// Cascade stage 1 (intermediate) for the pitch-family dests: `[dest][lane]`.
    p_stage1: [[f32; N]; N_PITCH],
    /// Cascade stage 2 (= smoothed output) for the pitch-family dests.
    p_state: [[f32; N]; N_PITCH],
    /// Per-oscillator PWM offsets, `[osc 1, osc 2]`. The three PWM dests are
    /// summed per oscillator *before* the one-pole (0261), so this stays two
    /// smoothers rather than three, and a patch routing only the combined `Pwm`
    /// feeds both the same target — identical to before the split.
    pwm: [LaneOnePole; 2],
    /// The `CrossModAmount` dest *offset* (0242). The patch's own
    /// `cross_mod_amount` is added on top by the render, so a patch with no
    /// route on the dest holds exactly zero here.
    xmod: LaneOnePole,
    /// The non-env Amp coefficient. The only quantity on the *per-frame* tier —
    /// see `amp_coeff`.
    amp_stat: LaneOnePole,
    /// The `Pan` dest (0260). Pan is a *position*, so unlike PWM/cross-mod there
    /// is no patch scalar riding on top — this is the whole value the render
    /// pans by.
    pan: LaneOnePole,
    /// Cascade coeff, calibrated at the *quantum* tick rate (`sr / PITCH_QUANTUM`).
    pitch_coeff: f32,
    /// Amp one-pole coeff, calibrated at the *per-frame* (base-sample) rate.
    amp_coeff: f32,
    /// PWM / cross-mod one-pole coeff, calibrated at the *quantum* tick rate.
    slow_coeff: f32,
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
        let slow_coeff = one_pole_coeff(SLOW_MS, sample_rate / PITCH_QUANTUM as f32);
        Self {
            p_stage1: [[0.0; N]; N_PITCH],
            p_state: [[0.0; N]; N_PITCH],
            pwm: Default::default(),
            xmod: LaneOnePole::default(),
            amp_stat: LaneOnePole::default(),
            pan: LaneOnePole::default(),
            pitch_coeff,
            amp_coeff,
            slow_coeff,
        }
    }

    /// Re-cook coeffs for a new sample rate; state is cleared to zero (callers
    /// reset the bank around a sample-rate change).
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        *self = Self::new(sample_rate);
    }

    /// Zero all state (bank reset). Coefficients are already cooked for the
    /// current sample rate, so only the state clears — and it clears wholesale,
    /// which is what stops a newly smoothed dest from being forgotten here.
    pub fn reset(&mut self) {
        self.p_stage1 = [[0.0; N]; N_PITCH];
        self.p_state = [[0.0; N]; N_PITCH];
        self.pwm = Default::default();
        self.xmod = LaneOnePole::default();
        self.amp_stat = LaneOnePole::default();
        self.pan = LaneOnePole::default();
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

    /// Snap **every** smoother for lane `v` to its block targets, so a fresh
    /// note starts settled: static sources (velocity, key) land zipper-free,
    /// and nothing glides in from the stolen voice's stale state.
    ///
    /// One entry point rather than three (0276) — a smoothed dest added without
    /// a matching snap would show up as a note-on glide from the previous
    /// voice's value, which is subtle enough to ship unnoticed.
    #[inline]
    pub(crate) fn snap_all(&mut self, v: usize, t: &crate::bank::LaneTargets) {
        self.snap_pitch(v, t.pitch, t.sweep);
        self.snap_slow(v, t.pwm, t.xmod, t.amp_stat);
        self.snap_pan(v, t.pan);
    }

    /// Snap one lane's PWM / cross-mod / Amp-stat one-poles to their block
    /// targets.
    #[inline]
    pub fn snap_slow(
        &mut self,
        v: usize,
        pwm_targets: (f32, f32),
        xmod_target: f32,
        amp_stat_target: f32,
    ) {
        self.pwm[0].snap(v, pwm_targets.0);
        self.pwm[1].snap(v, pwm_targets.1);
        self.xmod.snap(v, xmod_target);
        self.amp_stat.snap(v, amp_stat_target);
    }

    /// Snap one lane's pan one-pole (0260). Separate from [`Self::snap_slow`]
    /// because a *stolen* lane must not glide across the image from wherever
    /// the previous note sat — it starts where its own patch puts it.
    #[inline]
    pub fn snap_pan(&mut self, v: usize, target: f32) {
        self.pan.snap(v, target);
    }

    /// Whether this lane's pan is moving (or displaced), i.e. worth ticking.
    #[inline]
    pub fn pan_active(&self, v: usize, target: f32) -> bool {
        self.pan.active(v, target)
    }

    /// Advance one lane's pan one-pole a quantum step and return the new value.
    #[inline]
    pub fn tick_pan(&mut self, v: usize, target: f32) -> f32 {
        self.pan.tick(v, target, self.slow_coeff)
    }

    /// This lane's current smoothed pan without advancing it.
    #[inline]
    pub fn pan_current(&self, v: usize) -> f32 {
        self.pan.current(v)
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
    ///
    /// Both cascade *stages* are checked, not just the output: stage 1 can still
    /// hold energy the output has yet to see.
    #[inline]
    pub fn pitch_active(&self, v: usize, pitch_target: f32, sweep_target: f32) -> bool {
        pitch_target.abs() > SETTLE_EPS
            || sweep_target.abs() > SETTLE_EPS
            || self.p_state[PITCH][v].abs() > SETTLE_EPS
            || self.p_state[SWEEP][v].abs() > SETTLE_EPS
            || self.p_stage1[PITCH][v].abs() > SETTLE_EPS
            || self.p_stage1[SWEEP][v].abs() > SETTLE_EPS
    }

    /// Whether lane `v`'s PWM one-poles need ticking — on *either* oscillator
    /// (0261); when false the render loop keeps the block-start pulse widths. A
    /// patch with no PWM route holds zero on both and stays on the
    /// block-constant path exactly as before the split.
    #[inline]
    pub fn pwm_active(&self, v: usize, targets: (f32, f32)) -> bool {
        self.pwm[0].active(v, targets.0) || self.pwm[1].active(v, targets.1)
    }

    /// Advance lane `v`'s PWM one-poles one quantum step and return the smoothed
    /// `(osc 1, osc 2)` offsets.
    #[inline]
    pub fn tick_pwm(&mut self, v: usize, targets: (f32, f32)) -> (f32, f32) {
        (
            self.pwm[0].tick(v, targets.0, self.slow_coeff),
            self.pwm[1].tick(v, targets.1, self.slow_coeff),
        )
    }

    /// Lane `v`'s current smoothed `(osc 1, osc 2)` PWM offsets, without
    /// advancing (block-start peek).
    #[inline]
    pub fn pwm_current(&self, v: usize) -> (f32, f32) {
        (self.pwm[0].current(v), self.pwm[1].current(v))
    }

    /// Whether lane `v`'s cross-mod one-pole needs ticking; when false the
    /// render keeps the block-start PM index — and, with every lane inactive,
    /// stays on the broadcast PM kernel entirely.
    #[inline]
    pub fn xmod_active(&self, v: usize, target: f32) -> bool {
        self.xmod.active(v, target)
    }

    /// Advance lane `v`'s cross-mod one-pole one quantum step and return the
    /// smoothed PM-index *offset* (the patch amount is added by the render).
    #[inline]
    pub fn tick_xmod(&mut self, v: usize, target: f32) -> f32 {
        self.xmod.tick(v, target, self.slow_coeff)
    }

    /// Lane `v`'s current smoothed cross-mod offset, without advancing.
    #[inline]
    pub fn xmod_current(&self, v: usize) -> f32 {
        self.xmod.current(v)
    }

    /// Advance lane `v`'s non-env Amp one-pole one **frame** step and return the
    /// smoothed static Amp coefficient. Ticked per sample (not per quantum)
    /// because a block-held amplitude stair is itself an audible click on a slow
    /// carrier — hence `amp_coeff` rather than `slow_coeff`.
    #[inline]
    pub fn tick_amp_stat(&mut self, v: usize, target: f32) -> f32 {
        self.amp_stat.tick(v, target, self.amp_coeff)
    }

    /// Lane `v`'s current smoothed Amp coefficient, without advancing.
    #[inline]
    pub fn amp_stat_current(&self, v: usize) -> f32 {
        self.amp_stat.current(v)
    }

    /// True when lane `v`'s Amp one-pole has arrived at `target` — the render
    /// loop keeps its envelope-static constant-amp fast path only while this
    /// holds for every active lane.
    #[inline]
    pub fn amp_stat_settled(&self, v: usize, target: f32) -> bool {
        self.amp_stat.settled(v, target)
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
        let (first, _) = s.tick_pwm(0, (1.0, 1.0));
        assert!(first > 0.0 && first < 0.5, "per-quantum step is partial, got {first}");
        for _ in 0..256 {
            s.tick_pwm(0, (1.0, 1.0));
        }
        let (a, b) = s.tick_pwm(0, (1.0, 1.0));
        assert!((a - 1.0).abs() < 1e-2);
        assert_eq!(a, b, "equal targets ⇒ the two lanes track identically");
    }

    /// 0261: the two lanes are independent — osc 2 stays put while osc 1 moves,
    /// and the active gate fires while *either* is live.
    #[test]
    fn pwm_lanes_are_independent_and_gate_on_either() {
        let mut s = MotionSmoother::new(SR);
        assert!(!s.pwm_active(0, (0.0, 0.0)), "no route, zero state → inactive");
        assert!(s.pwm_active(0, (0.0, 0.4)), "osc 2 target alone arms the gate");
        for _ in 0..256 {
            s.tick_pwm(0, (0.3, 0.0));
        }
        let (a, b) = s.pwm_current(0);
        assert!((a - 0.3).abs() < 1e-2, "osc 1 settles at its target, got {a}");
        assert_eq!(b, 0.0, "osc 2 never moved");
        assert!(s.pwm_active(0, (0.0, 0.0)), "osc 1's residual keeps it active");
        // Snapping takes both lanes.
        s.snap_slow(0, (0.1, -0.2), 0.0, 0.0);
        assert_eq!(s.pwm_current(0), (0.1, -0.2));
    }

    #[test]
    fn xmod_one_pole_glides_settles_and_snaps() {
        let mut s = MotionSmoother::new(SR);
        assert!(!s.xmod_active(0, 0.0), "no route, zero state → inactive");
        let first = s.tick_xmod(0, 2.0);
        assert!(first > 0.0 && first < 1.0, "per-quantum step is partial, got {first}");
        assert!(s.xmod_active(0, 0.0), "residual energy keeps it active after the route drops");
        for _ in 0..256 {
            s.tick_xmod(0, 2.0);
        }
        assert!((s.xmod_current(0) - 2.0).abs() < 1e-2);
        // A fresh note snaps the lane so the index starts settled, not gliding
        // up from the stolen voice's state.
        s.snap_slow(0, (0.0, 0.0), 0.5, 0.0);
        assert!((s.tick_xmod(0, 0.5) - 0.5).abs() < 1e-6);
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
