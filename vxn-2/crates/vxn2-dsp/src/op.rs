//! The operator: one phase accumulator + sine generator + 4R/4L EG + per-op
//! level + key scaling + velocity / amp sens + per-op feedback. The atom of
//! every VXN2 voice — up to 6 ops per voice × 8 stack × 16 poly = 768 op
//! instances in flight.
//!
//! Split into two state spaces:
//!
//! - [`OpParams`] — plain user-facing values straight from `PARAMETERS.md`.
//!   Mutable by the host; the UI/CLAP layer writes here.
//! - [`OpState`] — runtime state: Q32 phase, cooked phase increment, feedback
//!   memory, EG. Re-cooked from `OpParams + key + velocity + sample_rate` on
//!   note-on or param change via [`OpState::cook`].
//!
//! Hot path is [`op_tick`]: per-sample, branch-free, single sine eval + two
//! FMAs + a phase add + a wrapping_add. EG advance is separate
//! ([`op_eg_tick`]) so the caller can run the EG at control rate (typically
//! once per block) and keep the per-sample loop tight.

use crate::eg::{EgParams, EgState};
use crate::ks::{KsCurve, ks_level_mult, ks_rate_mult};
use crate::sine;
use crate::tables::vel_factor;
#[cfg(test)]
use crate::tables::fb_scale;

/// Whether the rational ratio (`num`/`denom`) or `fixed_hz` drives the
/// operator's frequency.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum RatioMode {
    #[default]
    Ratio,
    Fixed,
}

#[derive(Clone, Copy, Debug)]
pub struct OpParams {
    pub ratio_mode: RatioMode,
    pub num: u8,
    pub denom: u8,
    pub fixed_hz: f32,
    pub fine: i8,
    pub detune: i8,
    pub level: u8,
    pub vel_sens: u8,
    pub eg: EgParams,
    pub ks_break_pt: u8,
    pub ks_l_depth: u8,
    pub ks_r_depth: u8,
    pub ks_l_curve: KsCurve,
    pub ks_r_curve: KsCurve,
    pub ks_rate: u8,
    pub pan: f32,
    /// Per-operator note-on phase offset, a fraction of one cycle in `[0, 1)`
    /// (1.0 = 2π). Composes additively (wrapping Q32) with the per-lane stack
    /// decorrelation offset at note-on so the six carriers of algo 32 can sum
    /// into specific analytic shapes (saw flips even harmonics by π = 0.5).
    /// Stack-path only — the scalar reference path does not reset phase, so the
    /// offset would wash out there. Ticket 0074.
    pub phase: f32,
}

impl Default for OpParams {
    /// Defaults per PARAMETERS.md (carrier-friendly: `level = 99`, EG sustains
    /// at L3, no key scaling cut).
    fn default() -> Self {
        Self {
            ratio_mode: RatioMode::Ratio,
            num: 1,
            denom: 1,
            fixed_hz: 440.0,
            fine: 0,
            detune: 0,
            level: 99,
            vel_sens: 3,
            eg: EgParams {
                r: [99, 50, 35, 60],
                l: [99, 70, 50, 0],
            },
            ks_break_pt: 60,
            ks_l_depth: 0,
            ks_r_depth: 30,
            ks_l_curve: KsCurve::NegLin,
            ks_r_curve: KsCurve::NegExp,
            ks_rate: 2,
            pan: 0.0,
            phase: 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct OpState {
    pub phase: u32,
    pub phase_inc: u32,
    pub fb_prev1: f32,
    pub fb_prev2: f32,
    pub fb_scale: f32,
    pub eg: EgState,
}

/// 2^32 — scales an f32 modulator in [-1, +1] to a Q32 phase offset (unit
/// modulator = one cycle = 2π radians of phase shift).
pub const PM_SCALE_Q32: f32 = 4_294_967_296.0;

/// MIDI note → frequency in Hz. A4 (MIDI 69) = 440 Hz.
#[inline]
pub fn midi_to_hz(note: u8) -> f32 {
    let n = note as f32;
    440.0 * 2_f32.powf((n - 69.0) / 12.0)
}

/// Operator base frequency (Hz) from its ratio/fixed mode and the played key,
/// *before* any per-lane stack detune. Ratio mode: `midi_to_hz(key)` times the
/// effective ratio (`num + fine/100` over `denom`) and the detune-cents factor;
/// Fixed mode: the literal `fixed_hz`. Single definition shared by
/// [`OpState::cook`] (scalar reference path) and `Stack::cook_op` (the
/// production per-lane path, which multiplies lane detune on top) — ticket 0071.
#[inline]
pub fn compute_base_hz(params: &OpParams, key: u8) -> f32 {
    match params.ratio_mode {
        RatioMode::Ratio => {
            let num_eff = params.num as f32 + (params.fine as f32) * 0.01;
            let denom = params.denom.max(1) as f32;
            let cents = params.detune as f32;
            midi_to_hz(key) * (num_eff / denom) * 2_f32.powf(cents / 1200.0)
        }
        RatioMode::Fixed => params.fixed_hz,
    }
}

impl OpState {
    /// Note-on / param-change cook. Re-derives `phase_inc`, EG targets/rates,
    /// and FB scale from `params + key + velocity + sample_rate`.
    /// Leaves `phase`, `fb_prev*` alone — caller can reset those separately
    /// if a clean note-on is wanted (see [`Self::reset_phase`]).
    pub fn cook(&mut self, params: &OpParams, key: u8, velocity: u8, sample_rate: f32) {
        let base_hz = compute_base_hz(params, key);
        self.phase_inc = ((base_hz / sample_rate) * PM_SCALE_Q32) as u32;

        let ks_lvl = ks_level_mult(
            key,
            params.ks_break_pt,
            params.ks_l_depth,
            params.ks_l_curve,
            params.ks_r_depth,
            params.ks_r_curve,
        );
        let vel = vel_factor(params.vel_sens, velocity);
        // Operator output level shares the EG level curve (DX7: OL and EG
        // levels live in the same log domain) — see `eg::level_to_amp`.
        let level_norm = crate::eg::level_to_amp(params.level);
        let max_amp = level_norm * ks_lvl * vel;
        let rate_mult = ks_rate_mult(key, params.ks_rate);
        self.eg.cook(&params.eg, max_amp, rate_mult);

        // Feedback is now layer-level: stack/voice note_on (and the engine's
        // per-block live update) writes `fb_scale` directly onto the
        // algorithm's structural FB op only. Leave it alone here.
    }

    /// Reset phase + feedback memory. Call on a clean note-on if the patch
    /// wants phase-aligned attacks across stack instances (otherwise leave
    /// phase free for the supersaw decorrelation).
    pub fn reset_phase(&mut self) {
        self.phase = 0;
        self.fb_prev1 = 0.0;
        self.fb_prev2 = 0.0;
    }

    /// Force into Sustain at a given level — fixture for steady-state benches
    /// and tests. Skips the attack/decay segments.
    pub fn force_sustain(&mut self, level: f32) {
        self.eg.stage = crate::eg::EgStage::Sustain;
        self.eg.level = level;
    }
}

/// Per-sample operator tick. Branch-free hot path.
///
/// - `mod_in` is the phase-modulation input in cycles: `1.0` = one full cycle
///   of phase shift (matches the [`PM_SCALE_Q32`] convention). Typical patches
///   pass modulator outputs scaled by send levels.
/// - The EG level (`state.eg.level`) is held constant across the call — the
///   caller advances it via [`op_eg_tick`] at control rate.
/// - Per-op feedback reads the average of the last two outputs (DX7
///   anti-aliasing convention) scaled by the cached `fb_scale`.
///
/// Returns the post-EG sample.
#[inline(always)]
pub fn op_tick(state: &mut OpState, mod_in: f32) -> f32 {
    let fb_avg = 0.5 * (state.fb_prev1 + state.fb_prev2);
    let total_mod = mod_in + fb_avg * state.fb_scale;
    let pm_q32 = (total_mod * PM_SCALE_Q32) as i32 as u32;
    let phase_mod = state.phase.wrapping_add(pm_q32);
    let out = sine::scalar::fast_sine_q32(phase_mod) * state.eg.level;
    state.fb_prev2 = state.fb_prev1;
    state.fb_prev1 = out;
    state.phase = state.phase.wrapping_add(state.phase_inc);
    out
}

/// Advance the EG one control tick (typically once per block). `dt` is
/// seconds since the previous tick.
#[inline]
pub fn op_eg_tick(state: &mut OpState, dt: f32) -> f32 {
    state.eg.tick(dt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cook_sets_phase_inc_to_a4_at_48k() {
        let params = OpParams::default();
        let mut state = OpState::default();
        state.cook(&params, 69, 100, 48_000.0);
        // 440 Hz at 48k: inc = 440 / 48000 * 2^32 ≈ 39_370_534.
        let want = ((440.0 / 48_000.0) * PM_SCALE_Q32) as u32;
        assert_eq!(state.phase_inc, want);
    }

    #[test]
    fn fixed_mode_ignores_key() {
        let mut params = OpParams::default();
        params.ratio_mode = RatioMode::Fixed;
        params.fixed_hz = 1000.0;
        let mut a = OpState::default();
        a.cook(&params, 36, 100, 48_000.0);
        let mut b = OpState::default();
        b.cook(&params, 96, 100, 48_000.0);
        assert_eq!(a.phase_inc, b.phase_inc);
    }

    #[test]
    fn op_tick_no_mod_no_fb_at_sustain_outputs_sine_scaled() {
        // Configure: A4, sustain forced at 0.5, no FB, no PM.
        let params = OpParams::default();
        let mut state = OpState::default();
        state.cook(&params, 69, 100, 48_000.0);
        state.force_sustain(0.5);
        // After 1/4 period (≈ 109 samples at 440 Hz), expect ≈ 0.5 (peak * 0.5).
        let mut out = 0.0;
        for _ in 0..(48_000 / 440 / 4) {
            out = op_tick(&mut state, 0.0);
        }
        assert!(out > 0.3, "expected near peak, got {out}");
    }

    #[test]
    fn op_tick_branch_free_no_panic_full_sweep() {
        // Smoke: 1 second at A3 (220) and at C8 (highest reasonable note),
        // varying modulation aggressively. No NaN, no overflow trap.
        for &(key, vel) in &[(57u8, 100u8), (108, 127)] {
            let params = OpParams::default();
            let mut state = OpState::default();
            state.cook(&params, key, vel, 48_000.0);
            state.fb_scale = fb_scale(7.0);
            state.eg.note_on();
            let dt_block = 64.0 / 48_000.0;
            for blk in 0..(48_000 / 64) {
                op_eg_tick(&mut state, dt_block);
                let modu = ((blk as f32) * 0.001).sin();
                for _ in 0..64 {
                    let s = op_tick(&mut state, modu);
                    assert!(s.is_finite(), "non-finite sample");
                }
            }
        }
    }

    #[test]
    fn cook_rational_ratio_three_halves_at_a4() {
        // num=3, denom=2, fine=0, detune=0 → 1.5 × 440 = 660 Hz.
        let params = OpParams {
            num: 3,
            denom: 2,
            ..OpParams::default()
        };
        let mut state = OpState::default();
        state.cook(&params, 69, 100, 48_000.0);
        let want = ((660.0 / 48_000.0) * PM_SCALE_Q32) as u32;
        assert!(state.phase_inc.abs_diff(want) <= 1, "{} vs {want}", state.phase_inc);
    }

    #[test]
    fn cook_fine_adds_hundredth_of_numerator() {
        // num=2, denom=1, fine=+50 → (2 + 0.5)/1 = 2.5 × 440 = 1100 Hz.
        let params = OpParams {
            num: 2,
            denom: 1,
            fine: 50,
            ..OpParams::default()
        };
        let mut state = OpState::default();
        state.cook(&params, 69, 100, 48_000.0);
        let want = ((1100.0 / 48_000.0) * PM_SCALE_Q32) as u32;
        assert!(state.phase_inc.abs_diff(want) <= 1, "{} vs {want}", state.phase_inc);
    }

    #[test]
    fn cook_detune_is_cents_one_for_one() {
        // detune=+100 ct → 1 semitone up → 440 × 2^(1/12) ≈ 466.164 Hz.
        let params = OpParams {
            detune: 100,
            ..OpParams::default()
        };
        let mut state = OpState::default();
        state.cook(&params, 69, 100, 48_000.0);
        let want_hz = 440.0 * 2_f32.powf(100.0 / 1200.0);
        let want = ((want_hz / 48_000.0) * PM_SCALE_Q32) as u32;
        assert!(state.phase_inc.abs_diff(want) <= 2, "{} vs {want}", state.phase_inc);
    }

    #[test]
    fn vel_sens_zero_amplitude_independent_of_velocity() {
        let params = OpParams {
            vel_sens: 0,
            ..OpParams::default()
        };
        let mut a = OpState::default();
        a.cook(&params, 60, 1, 48_000.0);
        let mut b = OpState::default();
        b.cook(&params, 60, 127, 48_000.0);
        // EG targets are equal → same max amp.
        assert!((a.eg.targets[0] - b.eg.targets[0]).abs() < 1e-6);
    }

    #[test]
    fn vel_sens_seven_attenuates_low_velocity() {
        let params = OpParams {
            vel_sens: 7,
            ..OpParams::default()
        };
        let mut a = OpState::default();
        a.cook(&params, 60, 1, 48_000.0);
        let mut b = OpState::default();
        b.cook(&params, 60, 127, 48_000.0);
        assert!(a.eg.targets[0] < 0.1 * b.eg.targets[0]);
    }

    #[test]
    fn feedback_alters_output_vs_no_feedback() {
        // Same note, same modulation; with FB the output should diverge.
        // Feedback is now layer-level; write `fb_scale` directly to match
        // how stack/voice note_on does it for the structural FB op.
        let params = OpParams::default();
        let mut a = OpState::default();
        a.cook(&params, 60, 100, 48_000.0);
        a.fb_scale = fb_scale(0.0);
        a.force_sustain(0.7);
        let mut b = OpState::default();
        b.cook(&params, 60, 100, 48_000.0);
        b.fb_scale = fb_scale(6.0);
        b.force_sustain(0.7);
        let mut differ = 0;
        for _ in 0..4096 {
            let sa = op_tick(&mut a, 0.0);
            let sb = op_tick(&mut b, 0.0);
            if (sa - sb).abs() > 1e-3 {
                differ += 1;
            }
        }
        assert!(differ > 100, "feedback had no audible effect");
    }

    #[test]
    fn feedback_fractional_value_distinct_from_neighbours() {
        // Continuous feedback: fb_scale(3.5) sits between fb_scale(3) and (4).
        // Run identical ops at the three settings; the 3.5 output must differ
        // from both integer neighbours.
        let params = OpParams::default();
        let mut a = OpState::default();
        let mut b = OpState::default();
        let mut c = OpState::default();
        for s in [&mut a, &mut b, &mut c] {
            s.cook(&params, 60, 100, 48_000.0);
            s.force_sustain(0.7);
        }
        a.fb_scale = fb_scale(3.0);
        b.fb_scale = fb_scale(3.5);
        c.fb_scale = fb_scale(4.0);
        let mut diff_ab = 0;
        let mut diff_bc = 0;
        for _ in 0..2048 {
            let sa = op_tick(&mut a, 0.0);
            let sb = op_tick(&mut b, 0.0);
            let sc = op_tick(&mut c, 0.0);
            if (sa - sb).abs() > 1e-3 {
                diff_ab += 1;
            }
            if (sb - sc).abs() > 1e-3 {
                diff_bc += 1;
            }
        }
        assert!(diff_ab > 50, "fb 3.0 vs 3.5 produced identical signal");
        assert!(diff_bc > 50, "fb 3.5 vs 4.0 produced identical signal");
    }
}
