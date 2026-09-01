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
//!   rationale, ported and trimmed from VXN2's 16 stack-lanes to one VXN1b
//!   render bank's [`RenderBank::LANES`](crate::bank::RenderBank::LANES)).
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
//! ## Which dest gets which tier is declared, and now *read*
//!
//! The tiers above used to live only in this prose and in the shape of
//! [`MotionSmoother`]'s fields — nothing tied them to the destinations they
//! smooth, so a new dest simply stairstepped until someone noticed. 0332 made
//! each destination's class the `smooth =` column of its row in
//! [`crate::matrix::DestId`]; **0335 made this module read it.** The cascade's
//! width and its row set are derived from the column
//! ([`vxn_core_matrix::smoothing::class_rows`]), and a `const` assert holds the
//! per-quantum bank to the `quantum` column, so declaring a new smoothed
//! destination is a build error here rather than a silent stairstep.
//!
//! The filters themselves are [`vxn_core_matrix::smoothing`]'s — the same
//! [`CascadeBank`] VXN2's pitch smoother is, and the same [`OnePoleBank`] under
//! all four of the one-pole quantities. What stays in this module is the
//! binding: which rows exist, which coefficient each bank is cooked at, and the
//! per-lane gating the render loop needs.
//!
//! Two mismatches between the column and this module are deliberate:
//!
//! * **`Amp` declares `block`.** The per-frame one-pole below is applied to the
//!   *non-envelope* part of the VCA coefficient only — the envelope part must
//!   stay per-frame exact or the attack smears. That factoring is a property of
//!   VXN1b's VCA, not of routing, so it keeps its own bank at its own rate and
//!   the roster declares the class the shared bank would apply to the whole
//!   total, which is none. The one acknowledged exception in ADR 0003 §3.
//! * **The three PWM dests share two smoothers.** `Pwm`, `Osc1Pwm` and
//!   `Osc2Pwm` are summed *per oscillator* before the one-pole, so three rows of
//!   one class land on two poles. Post-sum smoothing is linear, so that is
//!   arithmetically the same filter — a layout detail, not a third tier. The
//!   `const` assert above knows about it and would fail if a *fourth* `quantum`
//!   destination appeared without a row.
//!
//! ## Why the ticking stays per-lane
//!
//! VXN2 advances its whole stack every quantum. This synth advances **only
//! lanes with a live route**, and the same branch decides whether to re-cook
//! that lane's oscillator increment, pulse width, PM index or pan gains — tick
//! and cook are one test. Flattening to a bank-wide tick would advance lanes
//! that currently freeze, and on a pitch destination an ULP-scale difference
//! integrates into phase drift. The class is shared; the schedule is this
//! render loop's.
//!
//! All state is fixed-size `[f32; N]` per row — allocation-free, `Copy`,
//! NEON-friendly. A fresh note **snaps** its lane (both cascade stages and the
//! one-poles) to the block target so the voice starts settled: static sources
//! (velocity, key) land zipper-free and there's no glide from the stolen voice's
//! stale state.

use vxn_core_matrix::roster::Smoothing;
use vxn_core_matrix::smoothing::{CascadeBank, OnePoleBank, class_count, class_rows, row_of};
use vxn_dsp::one_pole_coeff;

use crate::matrix::{DEST_SMOOTHING, DestId};

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

/// Rows in the cascade bank — **derived** from the `smooth = quantum_cascade`
/// column, not written down (0335).
///
/// The count and the row set both come from
/// [`vxn_core_matrix::smoothing`], so declaring a new `quantum_cascade`
/// destination widens this bank rather than silently stairstepping. The order is
/// the roster's, which is why [`PITCH`] and [`SWEEP`] ask for their position by
/// name: a literal would be right until a cascade dest is declared ahead of them.
const N_PITCH: usize = class_count(&DEST_SMOOTHING, Smoothing::QuantumCascade);

/// Destination storage rows the cascade smooths, in roster order.
const PITCH_ROWS: [usize; N_PITCH] = class_rows(&DEST_SMOOTHING, Smoothing::QuantumCascade);

/// Cascade row carrying `dest`. A `const fn` wrapper so a bad name is a build
/// error at the `const` below rather than an `unwrap` in a lane loop.
const fn cascade_row(dest: DestId) -> usize {
    match dest.idx() {
        Some(d) => match row_of(&PITCH_ROWS, d) {
            Some(r) => r,
            None => panic!("this destination does not declare quantum_cascade"),
        },
        None => panic!("the sentinel is not a smoothed destination"),
    }
}

/// Cascade row for `Pitch` (both oscillators).
pub const PITCH: usize = cascade_row(DestId::Pitch);
/// Cascade row for `XModSweep` (the mode-gated oscillator sweep).
pub const SWEEP: usize = cascade_row(DestId::XModSweep);

/// Rows in the per-quantum one-pole bank.
///
/// **Not** one row per `quantum` destination, and that is a layout decision
/// rather than a drift: `Pwm`, `Osc1Pwm` and `Osc2Pwm` are summed *per
/// oscillator* before the pole (0261), so three declared rows land on two
/// smoothers. Post-sum smoothing is linear, so filtering the sum is the same
/// filter as filtering each and summing — the class is shared, the fan-in is
/// this synth's.
const PWM1: usize = 0;
const PWM2: usize = 1;
const XMOD: usize = 2;
const PAN: usize = 3;
const N_SLOW: usize = 4;

/// The per-quantum bank covers exactly the `quantum` destinations, with the
/// three PWM rows folded to two. Pinned so that declaring a new `quantum` dest
/// fails the build here instead of stairstepping unnoticed.
const _: () = {
    const N_QUANTUM: usize = class_count(&DEST_SMOOTHING, Smoothing::Quantum);
    assert!(
        N_SLOW == N_QUANTUM - 1,
        "the per-quantum bank is one row short of the `quantum` column, because the three \
         PWM dests share two poles. A new `quantum` destination needs a row here."
    );
};

/// Per-lane motion smoothers for one render bank.
///
/// **Three shared banks and nothing else** as of 0335: the recurrences, the
/// state, the snaps and the settle predicates are
/// [`vxn_core_matrix::smoothing`]'s, and what stays here is the binding —
/// which rows exist, which coefficient each bank is cooked at, and the
/// per-lane gating the render loop needs.
///
/// The gating is why this type still exists rather than the render loop holding
/// the banks directly. vxn-2 ticks its whole stack every quantum; vxn-1b ticks
/// **only lanes with a live route**, and the same branch decides whether to
/// re-cook that lane's oscillator increment, pulse width, PM index or pan gains.
/// Advancing every lane instead would move state that currently freezes, and on
/// a pitch destination an ULP-scale difference integrates into phase drift.
#[derive(Clone, Copy, Debug)]
pub struct MotionSmoother {
    /// The `quantum_cascade` bank: `Pitch` and `XModSweep`, two poles each,
    /// ticked per [`PITCH_QUANTUM`] samples.
    pitch: CascadeBank<N_PITCH, N>,
    /// The `quantum` bank: the two per-oscillator PWM offsets, the
    /// `CrossModAmount` offset and `Pan`, one pole each, ticked per quantum.
    ///
    /// One bank rather than four smoothers because they share a coefficient —
    /// the coefficient belongs to the class and the tick rate, not to the
    /// quantity.
    slow: OnePoleBank<N_SLOW, N>,
    /// The non-envelope Amp coefficient — **not** a [`Smoothing`] class, and
    /// the one acknowledged exception in ADR 0003 §3.
    ///
    /// `Amp` declares `block`, because what the shared bank would smooth is a
    /// destination's whole total and that is not what happens here: only the
    /// *static* part of the VCA coefficient is filtered, while the envelope part
    /// stays per-frame exact or the attack smears. That factoring is a property
    /// of this synth's VCA rather than of routing, so it keeps its own bank at
    /// its own (per-frame) rate. It is a deliberate limit on the abstraction,
    /// not a gap.
    amp_stat: OnePoleBank<1, N>,
}

/// The single row of [`MotionSmoother::amp_stat`].
const AMP: usize = 0;

impl MotionSmoother {
    /// `sample_rate` is the base (non-oversampled) rate the render loop runs at.
    ///
    /// Each bank is cooked at the rate **it** is ticked at, which is the whole
    /// reason the coefficient is the caller's to supply: pitch and the slow
    /// group advance once per [`PITCH_QUANTUM`] samples, Amp every frame, and
    /// the same time constant needs a different coefficient at each.
    pub fn new(sample_rate: f32) -> Self {
        // Cascade time constant ≈ one control block, but ticked per quantum, so
        // calibrate the coeff at the quantum rate (matches VXN2).
        let block_ms = vxn_dsp::CONTROL_BLOCK as f32 / sample_rate * 1000.0;
        let quantum_rate = sample_rate / PITCH_QUANTUM as f32;
        Self {
            pitch: CascadeBank::new(one_pole_coeff(block_ms, quantum_rate)),
            slow: OnePoleBank::new(one_pole_coeff(SLOW_MS, quantum_rate)),
            // Amp glides every sample; see the field's note on why.
            amp_stat: OnePoleBank::new(one_pole_coeff(SLOW_MS, sample_rate)),
        }
    }

    /// Zero all state (bank reset). Coefficients are already cooked for the
    /// current sample rate, so only the state clears — and it clears wholesale,
    /// which is what stops a newly smoothed dest from being forgotten here.
    pub fn reset(&mut self) {
        self.pitch.clear();
        self.slow.clear();
        self.amp_stat.clear();
    }

    /// Snap one lane's pitch cascade (both stages) to the block targets, so a
    /// fresh note starts settled rather than gliding up from the previous voice.
    #[inline]
    pub fn snap_pitch(&mut self, v: usize, pitch_target: f32, sweep_target: f32) {
        self.pitch.snap_lane(PITCH, v, pitch_target);
        self.pitch.snap_lane(SWEEP, v, sweep_target);
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
        self.slow.snap_lane(PWM1, v, pwm_targets.0);
        self.slow.snap_lane(PWM2, v, pwm_targets.1);
        self.slow.snap_lane(XMOD, v, xmod_target);
        self.amp_stat.snap_lane(AMP, v, amp_stat_target);
    }

    /// Snap one lane's pan one-pole (0260). Separate from [`Self::snap_slow`]
    /// because a *stolen* lane must not glide across the image from wherever the
    /// previous note sat — it starts where its own patch puts it.
    #[inline]
    pub fn snap_pan(&mut self, v: usize, target: f32) {
        self.slow.snap_lane(PAN, v, target);
    }

    /// Whether this lane's pan is moving (or displaced), i.e. worth ticking.
    #[inline]
    pub fn pan_active(&self, v: usize, target: f32) -> bool {
        self.slow.lane_active(PAN, v, target, SETTLE_EPS)
    }

    /// Advance one lane's pan one-pole a quantum step and return the new value.
    #[inline]
    pub fn tick_pan(&mut self, v: usize, target: f32) -> f32 {
        self.slow.tick_lane(PAN, v, target)
    }

    /// This lane's current smoothed pan without advancing it.
    #[inline]
    pub fn pan_current(&self, v: usize) -> f32 {
        self.slow.current_lane(PAN, v)
    }

    /// Whether lane `v`'s cross-mod one-pole needs ticking; when false the
    /// render keeps the block-start PM index — and, with every lane inactive,
    /// stays on the broadcast PM kernel entirely.
    #[inline]
    pub fn xmod_active(&self, v: usize, target: f32) -> bool {
        self.slow.lane_active(XMOD, v, target, SETTLE_EPS)
    }

    /// Advance lane `v`'s cross-mod one-pole one quantum step and return the
    /// smoothed PM-index *offset* (the patch amount is added by the render).
    #[inline]
    pub fn tick_xmod(&mut self, v: usize, target: f32) -> f32 {
        self.slow.tick_lane(XMOD, v, target)
    }

    /// Lane `v`'s current smoothed cross-mod offset, without advancing.
    #[inline]
    pub fn xmod_current(&self, v: usize) -> f32 {
        self.slow.current_lane(XMOD, v)
    }

    /// Advance lane `v`'s non-env Amp one-pole one **frame** step and return the
    /// smoothed static Amp coefficient. Ticked per sample (not per quantum)
    /// because a block-held amplitude stair is itself an audible click on a slow
    /// carrier — hence its own bank at the frame rate.
    #[inline]
    pub fn tick_amp_stat(&mut self, v: usize, target: f32) -> f32 {
        self.amp_stat.tick_lane(AMP, v, target)
    }

    /// Lane `v`'s current smoothed Amp coefficient, without advancing.
    #[inline]
    pub fn amp_stat_current(&self, v: usize) -> f32 {
        self.amp_stat.current_lane(AMP, v)
    }

    /// True when lane `v`'s Amp one-pole has arrived at `target` — the render
    /// loop keeps its envelope-static constant-amp fast path only while this
    /// holds for every active lane.
    #[inline]
    pub fn amp_stat_settled(&self, v: usize, target: f32) -> bool {
        self.amp_stat.lane_settled(AMP, v, target, SETTLE_EPS)
    }

    /// Advance one lane's pitch cascade one quantum step toward the targets and
    /// return the smoothed `(pitch, sweep)` offsets. Stage 1 chases the target;
    /// stage 2 (the output) chases stage 1 — the second stage is what gives a
    /// zero starting slope so sharp LFO-into-pitch steps ramp in without a click.
    #[inline]
    pub fn tick_pitch(&mut self, v: usize, pitch_target: f32, sweep_target: f32) -> (f32, f32) {
        (
            self.pitch.tick_lane(PITCH, v, pitch_target),
            self.pitch.tick_lane(SWEEP, v, sweep_target),
        )
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
        self.pitch.lane_active(PITCH, v, pitch_target, SETTLE_EPS)
            || self.pitch.lane_active(SWEEP, v, sweep_target, SETTLE_EPS)
    }

    /// Whether lane `v`'s PWM one-poles need ticking — on *either* oscillator
    /// (0261); when false the render loop keeps the block-start pulse widths. A
    /// patch with no PWM route holds zero on both and stays on the
    /// block-constant path exactly as before the split.
    #[inline]
    pub fn pwm_active(&self, v: usize, targets: (f32, f32)) -> bool {
        self.slow.lane_active(PWM1, v, targets.0, SETTLE_EPS)
            || self.slow.lane_active(PWM2, v, targets.1, SETTLE_EPS)
    }

    /// Advance lane `v`'s PWM one-poles one quantum step and return the smoothed
    /// `(osc 1, osc 2)` offsets.
    #[inline]
    pub fn tick_pwm(&mut self, v: usize, targets: (f32, f32)) -> (f32, f32) {
        (
            self.slow.tick_lane(PWM1, v, targets.0),
            self.slow.tick_lane(PWM2, v, targets.1),
        )
    }

    /// Lane `v`'s current smoothed `(osc 1, osc 2)` PWM offsets, without
    /// advancing (block-start peek).
    #[inline]
    pub fn pwm_current(&self, v: usize) -> (f32, f32) {
        (self.slow.current_lane(PWM1, v), self.slow.current_lane(PWM2, v))
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
