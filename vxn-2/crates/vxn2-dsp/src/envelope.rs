//! Patch-wide modulation envelopes beyond the per-op EG.
//!
//! Two flavours share segment-march helpers with each other but not with the
//! per-op [`crate::eg`] — the per-op EG is the time-critical hot path (every
//! op × stack × poly ticks one per block) and stays self-contained. These
//! envelopes run once per stack per block and are not on the same budget.
//!
//! - [`PitchEgState`] — 4-rate / 4-level with *signed* levels in
//!   `[−1, +1]` (mapped from the −99..+99 plain range). Output is in
//!   semitones, scaled by `peg_depth`. Default routing: additive into the
//!   voice's pitch sum (handled by [`crate::voice`] / [`crate::stack`]).
//! - [`ModEnvState`] — 4-stage ADSR with shape selector (Lin / Exp).
//!   General-purpose matrix source in `[0, 1]`. No default routing.
//!
//! Both retrigger on note-on and release on note-off. State is per-stack
//! (shared across lanes) — same precedent as the per-op EG. Matrix routing
//! can apply per-lane scaling at consumption time.
//!
//! ## Rate semantics
//!
//! Pitch EG R values use their own DX7-calibrated law
//! ([`rate_to_pitch_per_sec`]) — an absolute semitones-per-second march speed,
//! *not* the per-op EG's amplitude law. The two are unrelated curves on the
//! hardware and reusing the amp law made high R values sweep ~60× too fast.
//! Mod Env A/D/R are in milliseconds.
//!
//! ## Segment shape
//!
//! - Linear segments: level marches toward target at a fixed slope, segment
//!   finishes on exact reach.
//! - Exponential segments (Mod Env only): one-pole approach with
//!   `tau = secs / 4.6` (segment reaches ~99% of target in the specified
//!   time). Segment finishes when level is within `EXP_FINISH_EPS` of target.

/// DX7 pitch-EG rate table: R (0..99) → level-units added per envelope tick.
/// One level-unit is 1/32 octave and the tick cadence works out to `21.3` ticks
/// per second-of-unit-slope, so R marches at `TAB[R] / 21.3` octaves/sec
/// regardless of how far it has to travel. Transcribed from the DX7 behaviour
/// model (`pitchenv_rate`); distinct from the per-op amplitude rate curve.
const DX7_PEG_RATE: [u8; 100] = [
    1, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13, 14, 14, 15, 16,
    16, 17, 18, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 30, 31, 33, 34, 36, 37, 38, 39, 41, 42,
    44, 46, 47, 49, 51, 53, 54, 56, 58, 60, 62, 64, 66, 68, 70, 72, 74, 76, 79, 82, 85, 88, 91, 94,
    98, 102, 106, 110, 115, 120, 125, 130, 135, 141, 147, 153, 159, 165, 171, 178, 185, 193, 202,
    211, 232, 243, 254, 255,
];

/// Pitch EG march speed in **semitones per second** for a rate value `0..99`.
///
/// `TAB[R] / 21.3` octaves/sec × 12 → ≈ 0.56 st/s at R=0 (a 4-octave sweep
/// takes ~85 s) up to ≈ 144 st/s at R=99 (the same sweep in ~0.33 s). Unlike
/// the per-op amplitude rates this is an *absolute* speed: a short excursion
/// finishes proportionally sooner, which is exactly how the hardware behaves
/// (the DX7 marches a fixed number of pitch units per tick).
#[inline]
pub fn rate_to_pitch_per_sec(rate: u8) -> f32 {
    const ST_PER_TICK_UNIT: f32 = 12.0 / 21.3;
    DX7_PEG_RATE[rate.min(99) as usize] as f32 * ST_PER_TICK_UNIT
}

/// 4-segment stage enum (DX-style) — used by Pitch EG.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EnvStage4 {
    #[default]
    Idle,
    Attack,
    Decay1,
    Decay2,
    Sustain,
    Release,
}

/// ADSR stage enum — used by Mod Env.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AdsrStage {
    #[default]
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
}

/// Mod Env segment shape.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AdsrShape {
    #[default]
    Lin,
    Exp,
}

/// Pitch EG patch params. Levels are signed: `-99..+99` maps to `[-1, +1]`
/// before scaling by `peg_depth` to semitones.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PitchEgParams {
    pub r: [u8; 4],
    pub l: [i8; 4],
}

impl Default for PitchEgParams {
    /// L = 0 across the board → centred (silent) by default; the matrix can
    /// route around this even with defaults.
    fn default() -> Self {
        Self {
            r: [99, 50, 35, 60],
            l: [0, 0, 0, 0],
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct PitchEgState {
    pub stage: EnvStage4,
    /// Current output in semitones.
    pub level_st: f32,
    targets_st: [f32; 4],
    rates_st_per_sec: [f32; 4],
}

impl PitchEgState {
    /// Cook into per-segment slopes + targets in semitones. `depth_semitones`
    /// is the full-scale swing (`peg_depth`) — so `l=+99` → target =
    /// `+depth_semitones`. `rate_mult` is the optional key-rate scaler
    /// (typically 1.0 for the Pitch EG; the patch-level PEG has no per-op KS).
    ///
    /// Only the *targets* scale by `depth_semitones` — the march rate is the
    /// absolute DX7 speed in semitones/sec ([`rate_to_pitch_per_sec`]). A
    /// shallower `peg_depth` therefore also traverses sooner, matching the
    /// hardware, where the envelope has no depth control and a small level
    /// excursion is simply a shorter march at the same slope.
    pub fn cook(&mut self, params: &PitchEgParams, depth_semitones: f32, rate_mult: f32) {
        for i in 0..4 {
            let signed = (params.l[i].clamp(-99, 99) as f32) / 99.0;
            self.targets_st[i] = signed * depth_semitones;
            self.rates_st_per_sec[i] = rate_to_pitch_per_sec(params.r[i]) * rate_mult;
        }
    }

    /// Multiply every cooked segment rate by `scale` — the per-lane
    /// `pitch-eg-rate` mod factor. Applied after [`cook`](Self::cook), so
    /// `scale > 1` sweeps faster, `< 1` slower; targets are untouched. `1.0` is a
    /// bit-exact no-op.
    #[inline]
    pub fn scale_rates(&mut self, scale: f32) {
        for r in &mut self.rates_st_per_sec {
            *r *= scale;
        }
    }

    /// Trigger Attack. Level continues from wherever it is (retrigger-safe).
    #[inline]
    pub fn note_on(&mut self) {
        self.stage = EnvStage4::Attack;
    }

    /// Move to Release from any non-Idle stage.
    #[inline]
    pub fn note_off(&mut self) {
        if self.stage != EnvStage4::Idle {
            self.stage = EnvStage4::Release;
        }
    }

    /// Advance one control tick. Returns the current semitone offset.
    pub fn tick(&mut self, dt: f32) -> f32 {
        match self.stage {
            EnvStage4::Idle => {
                self.level_st = self.targets_st[3];
            }
            EnvStage4::Attack => self.march(0, EnvStage4::Decay1, dt),
            EnvStage4::Decay1 => self.march(1, EnvStage4::Decay2, dt),
            EnvStage4::Decay2 => self.march(2, EnvStage4::Sustain, dt),
            EnvStage4::Sustain => {}
            EnvStage4::Release => self.march(3, EnvStage4::Idle, dt),
        }
        self.level_st
    }

    #[inline]
    fn march(&mut self, idx: usize, next: EnvStage4, dt: f32) {
        if march_linear(
            &mut self.level_st,
            self.targets_st[idx],
            self.rates_st_per_sec[idx],
            dt,
        ) {
            self.stage = next;
        }
    }
}

// Mod Env
#[derive(Clone, Copy, Debug)]
pub struct ModEnvParams {
    pub a_ms: f32,
    pub d_ms: f32,
    pub s: f32,
    pub r_ms: f32,
    pub shape: AdsrShape,
}

impl Default for ModEnvParams {
    fn default() -> Self {
        Self {
            a_ms: 2.0,
            d_ms: 320.0,
            s: 0.60,
            r_ms: 180.0,
            shape: AdsrShape::Lin,
        }
    }
}

/// Mod Env runtime state. Cooked fields encode per-segment slopes (Lin)
/// or time constants (Exp).
#[derive(Clone, Copy, Debug, Default)]
pub struct ModEnvState {
    pub stage: AdsrStage,
    pub level: f32,
    pub shape: AdsrShape,
    pub sustain: f32,
    /// Lin: slope (1/sec). Exp: time constant `tau` in secs.
    attack: f32,
    decay: f32,
    release: f32,
}

/// `5τ ≈ 99.3%`; use `ln(100) ≈ 4.6` so the segment reaches ~99% of target
/// within the specified time.
const EXP_TAU_DIV: f32 = 4.6;
/// "Reached target" threshold for Exp segments. The asymptotic approach
/// never exactly hits the target; we declare a segment done when within this.
const EXP_FINISH_EPS: f32 = 1e-4;

impl ModEnvState {
    pub fn cook(&mut self, params: &ModEnvParams) {
        let a_s = params.a_ms.max(0.0) * 0.001;
        let d_s = params.d_ms.max(0.0) * 0.001;
        let r_s = params.r_ms.max(0.0) * 0.001;
        self.sustain = params.s.clamp(0.0, 1.0);
        self.shape = params.shape;
        match params.shape {
            AdsrShape::Lin => {
                // Slope sized so the segment traverses its full delta in the
                // given time. Attack: 0 → 1 (delta = 1). Decay: 1 → s
                // (delta = 1 - s). Release: s → 0 (delta = s). Cap to a
                // very large finite value so dt=0 isn't a divide.
                self.attack = inv_or_inf(a_s);
                self.decay = (1.0 - self.sustain).max(0.0) * inv_or_inf(d_s);
                self.release = self.sustain * inv_or_inf(r_s);
            }
            AdsrShape::Exp => {
                self.attack = (a_s / EXP_TAU_DIV).max(1e-6);
                self.decay = (d_s / EXP_TAU_DIV).max(1e-6);
                self.release = (r_s / EXP_TAU_DIV).max(1e-6);
            }
        }
    }

    /// Scale the env speed by `scale` — the per-voice `mod-env-rate` mod factor.
    /// `scale > 1` runs the ADSR faster, `< 1` slower. Because this is a
    /// *time*-based env, the internal representation differs by shape: `Lin`
    /// stores slopes (units/sec, bigger = faster → multiply) and `Exp` stores
    /// time constants (seconds, smaller = faster → divide). `1.0` is a bit-exact
    /// no-op. Applied after [`cook`](Self::cook).
    #[inline]
    pub fn scale_rates(&mut self, scale: f32) {
        if scale <= 0.0 {
            return;
        }
        match self.shape {
            AdsrShape::Lin => {
                self.attack *= scale;
                self.decay *= scale;
                self.release *= scale;
            }
            AdsrShape::Exp => {
                let inv = 1.0 / scale;
                self.attack = (self.attack * inv).max(1e-6);
                self.decay = (self.decay * inv).max(1e-6);
                self.release = (self.release * inv).max(1e-6);
            }
        }
    }

    #[inline]
    pub fn note_on(&mut self) {
        // Restart the attack from zero. The cooked Lin attack slope (`1/a_s`)
        // and the Exp attack tau are both sized for a full 0 → 1 traversal, so
        // a retrigger must reset the level: otherwise the attack marches from
        // the residual level (e.g. sustain) and finishes in a fraction of the
        // set time — the "attack does nothing on repeated notes" bug. This is a
        // *time*-based env (knobs in ms), unlike the rate-based op amp EG which
        // deliberately continues from the current level for click-free
        // retrigger. Legato continuation skips note_on entirely (see alloc.rs),
        // so this only fires on real retriggers.
        self.stage = AdsrStage::Attack;
        self.level = 0.0;
    }

    #[inline]
    pub fn note_off(&mut self) {
        if self.stage != AdsrStage::Idle {
            self.stage = AdsrStage::Release;
        }
    }

    pub fn tick(&mut self, dt: f32) -> f32 {
        match self.stage {
            AdsrStage::Idle => {
                self.level = 0.0;
            }
            AdsrStage::Attack => {
                let done = match self.shape {
                    AdsrShape::Lin => march_linear(&mut self.level, 1.0, self.attack, dt),
                    AdsrShape::Exp => march_exp(&mut self.level, 1.0, self.attack, dt),
                };
                if done {
                    self.stage = AdsrStage::Decay;
                }
            }
            AdsrStage::Decay => {
                let done = match self.shape {
                    AdsrShape::Lin => {
                        march_linear(&mut self.level, self.sustain, self.decay, dt)
                    }
                    AdsrShape::Exp => march_exp(&mut self.level, self.sustain, self.decay, dt),
                };
                if done {
                    self.stage = AdsrStage::Sustain;
                }
            }
            AdsrStage::Sustain => {}
            AdsrStage::Release => {
                let done = match self.shape {
                    AdsrShape::Lin => march_linear(&mut self.level, 0.0, self.release, dt),
                    AdsrShape::Exp => march_exp(&mut self.level, 0.0, self.release, dt),
                };
                if done {
                    self.stage = AdsrStage::Idle;
                }
            }
        }
        self.level
    }
}

// Segment march helpers.
#[inline]
fn inv_or_inf(secs: f32) -> f32 {
    if secs > 1e-6 {
        1.0 / secs
    } else {
        f32::INFINITY
    }
}

/// Linear segment march: advance `level` toward `target` at `rate` units/sec.
/// Returns `true` when the target is reached. Handles target-above and
/// target-below symmetrically. `rate = INFINITY` snaps to target.
#[inline]
fn march_linear(level: &mut f32, target: f32, rate: f32, dt: f32) -> bool {
    if !rate.is_finite() {
        *level = target;
        return true;
    }
    let step = rate * dt;
    if *level < target {
        *level += step;
        if *level >= target {
            *level = target;
            return true;
        }
    } else if *level > target {
        *level -= step;
        if *level <= target {
            *level = target;
            return true;
        }
    } else {
        return true;
    }
    false
}

/// Exponential segment march: one-pole approach to `target` with time
/// constant `tau_secs`. Returns `true` when within [`EXP_FINISH_EPS`] of
/// target.
#[inline]
fn march_exp(level: &mut f32, target: f32, tau_secs: f32, dt: f32) -> bool {
    let alpha = 1.0 - (-dt / tau_secs).exp();
    *level += (target - *level) * alpha;
    (*level - target).abs() < EXP_FINISH_EPS
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util;

    const DT: f32 = 64.0 / 48_000.0;

    #[test]
    fn pitch_eg_default_idle_zero() {
        let mut eg = PitchEgState::default();
        eg.cook(&PitchEgParams::default(), 1.0, 1.0);
        // Tick from Idle → reads target[3] = 0.
        let s = eg.tick(DT);
        assert_eq!(s, 0.0);
    }

    #[test]
    fn pitch_eg_signed_sweep_up_then_back() {
        // L1 = +50 (target ≈ +0.505), L2..L4 = 0. R1/R2 fast. Note-on,
        // expect positive peak then back toward 0.
        let mut eg = PitchEgState::default();
        let params = PitchEgParams {
            r: [99, 99, 99, 99],
            l: [50, 0, 0, 0],
        };
        eg.cook(&params, 1.0, 1.0);
        eg.note_on();
        let mut peak = 0.0_f32;
        let mut reached_decay2 = false;
        for _ in 0..2000 {
            let v = eg.tick(DT);
            if v > peak {
                peak = v;
            }
            if eg.stage == EnvStage4::Decay2 {
                reached_decay2 = true;
            }
            if reached_decay2 && v.abs() < 0.01 {
                break;
            }
        }
        assert!(peak > 0.45, "peak {peak} should be near +0.5");
        // After Decay2 with L3=0, level should head toward 0.
        assert!(eg.level_st.abs() < 0.05, "did not settle: {}", eg.level_st);
    }

    #[test]
    fn pitch_eg_negative_levels_sweep_down() {
        let mut eg = PitchEgState::default();
        let params = PitchEgParams {
            r: [99, 99, 99, 99],
            l: [-99, 0, 0, 0],
        };
        eg.cook(&params, 1.0, 1.0);
        eg.note_on();
        let mut min = 0.0_f32;
        for _ in 0..2000 {
            let v = eg.tick(DT);
            if v < min {
                min = v;
            }
        }
        assert!(min < -0.95, "min {min} should reach near −1");
    }

    #[test]
    fn pitch_eg_depth_scales_full_range() {
        // l = +99, depth = 2.0 → peak should reach +2 semitones.
        let mut eg = PitchEgState::default();
        let params = PitchEgParams {
            r: [99, 99, 99, 99],
            l: [99, 99, 99, 99],
        };
        eg.cook(&params, 2.0, 1.0);
        eg.note_on();
        for _ in 0..2000 {
            eg.tick(DT);
        }
        assert!(
            (eg.level_st - 2.0).abs() < 1e-3,
            "level {} should be 2.0",
            eg.level_st
        );
    }

    #[test]
    fn pitch_eg_rate_law_matches_dx7() {
        // TAB[R]/21.3 octaves/sec: R=0 → 1 unit, R=50 → 41, R=99 → 255.
        assert!((rate_to_pitch_per_sec(0) - 12.0 / 21.3).abs() < 1e-4);
        assert!((rate_to_pitch_per_sec(50) - 41.0 * 12.0 / 21.3).abs() < 1e-3);
        assert!((rate_to_pitch_per_sec(99) - 255.0 * 12.0 / 21.3).abs() < 1e-2);
        // Out-of-range clamps to R=99 rather than panicking on the table index.
        assert_eq!(rate_to_pitch_per_sec(200), rate_to_pitch_per_sec(99));
    }

    #[test]
    fn pitch_eg_traversal_time_is_absolute() {
        // The DX7 'VOICE 1' attack segment: R1 = 18 (TAB 11 → 6.20 st/s) over a
        // ~0.75 st dip → ~121 ms. Two depth/level encodings of that same
        // excursion take the same time, and the slope itself is depth-free.
        let mut slopes = [0.0_f32; 2];
        for (i, (depth, l1)) in [(0.75_f32, -99_i8), (1.5, -50)].into_iter().enumerate() {
            let mut eg = PitchEgState::default();
            eg.cook(&PitchEgParams { r: [18, 60, 95, 60], l: [l1, 0, 0, 0] }, depth, 1.0);
            eg.note_on();
            // First tick is a full un-clamped march step → the cooked slope.
            slopes[i] = eg.tick(DT).abs() / DT;
            let mut ticks = 1;
            while eg.stage == EnvStage4::Attack && ticks < 4000 {
                eg.tick(DT);
                ticks += 1;
            }
            let secs = ticks as f32 * DT;
            assert!(
                (secs - 0.121).abs() < 0.01,
                "depth {depth} l1 {l1}: attack took {secs}s, want ~0.121s"
            );
        }
        assert!(
            (slopes[0] - slopes[1]).abs() < 1e-3,
            "slope varied with depth: {slopes:?}"
        );
        assert!((slopes[0] - rate_to_pitch_per_sec(18)).abs() < 1e-3);
    }

    #[test]
    fn pitch_eg_release_returns_to_l4() {
        let mut eg = PitchEgState::default();
        let params = PitchEgParams {
            r: [99, 99, 99, 99],
            l: [50, 50, 50, -10],
        };
        eg.cook(&params, 1.0, 1.0);
        eg.note_on();
        test_util::run_until_stage(|| { eg.tick(DT); eg.stage == EnvStage4::Sustain }, 2000);
        eg.note_off();
        test_util::run_until_stage(|| { eg.tick(DT); eg.stage == EnvStage4::Idle }, 2000);
        // Idle holds L4 target.
        let expected = -10.0 / 99.0;
        assert!(
            (eg.level_st - expected).abs() < 1e-3,
            "L4 settle: {} want {}",
            eg.level_st,
            expected
        );
    }

    #[test]
    fn mod_env_lin_reaches_peak_and_sustain() {
        let mut env = ModEnvState::default();
        let params = ModEnvParams {
            a_ms: 10.0,
            d_ms: 20.0,
            s: 0.5,
            r_ms: 10.0,
            shape: AdsrShape::Lin,
        };
        env.cook(&params);
        env.note_on();
        let mut peak = 0.0_f32;
        test_util::run_until_stage(
            || {
                let v = env.tick(DT);
                if v > peak { peak = v; }
                env.stage == AdsrStage::Sustain
            },
            1000,
        );
        assert!(peak >= 0.99, "lin attack peak {peak}");
        assert!(
            (env.level - 0.5).abs() < 1e-3,
            "sustain {} want 0.5",
            env.level
        );
    }

    #[test]
    fn mod_env_exp_approaches_target() {
        let mut env = ModEnvState::default();
        let params = ModEnvParams {
            a_ms: 5.0,
            d_ms: 5.0,
            s: 0.3,
            r_ms: 5.0,
            shape: AdsrShape::Exp,
        };
        env.cook(&params);
        env.note_on();
        // Run long enough for Attack + Decay to finish (5 ms each ≈ tau×4.6).
        test_util::run_until_stage(|| { env.tick(DT); env.stage == AdsrStage::Sustain }, 500);
        assert_eq!(env.stage, AdsrStage::Sustain);
        assert!(
            (env.level - 0.3).abs() < 1e-2,
            "exp sustain {} want ≈0.3",
            env.level
        );
    }

    #[test]
    fn mod_env_release_falls_to_zero() {
        let mut env = ModEnvState::default();
        let params = ModEnvParams {
            a_ms: 1.0,
            d_ms: 1.0,
            s: 0.5,
            r_ms: 5.0,
            shape: AdsrShape::Lin,
        };
        env.cook(&params);
        env.note_on();
        test_util::run_until_stage(|| { env.tick(DT); env.stage == AdsrStage::Sustain }, 1000);
        env.note_off();
        test_util::run_until_stage(|| { env.tick(DT); env.stage == AdsrStage::Idle }, 1000);
        assert_eq!(env.stage, AdsrStage::Idle);
        assert!(env.level.abs() < 1e-3);
    }

    #[test]
    fn mod_env_zero_attack_snaps_to_peak() {
        // a_ms = 0 → infinite slope → first tick reaches 1.0.
        let mut env = ModEnvState::default();
        let params = ModEnvParams {
            a_ms: 0.0,
            d_ms: 1000.0,
            s: 0.5,
            r_ms: 100.0,
            shape: AdsrShape::Lin,
        };
        env.cook(&params);
        env.note_on();
        env.tick(DT);
        // Attack snaps to 1.0 then moves into Decay; level is at 1.0 minus
        // a single decay step (small).
        assert!(env.level >= 0.9, "level after snap {}", env.level);
    }

    /// Retrigger must restart the attack from zero. The cooked Lin slope is
    /// sized for a full 0 → 1 sweep, so a `note_on` from a residual level (here
    /// the sustain plateau) must reset to 0 — otherwise the second attack
    /// marches from sustain and finishes almost instantly, the "attack does
    /// nothing on repeated notes" bug. We assert (a) the level snaps to 0 on
    /// retrigger and (b) the second attack takes essentially the full time
    /// again, matching the first.
    #[test]
    fn mod_env_retrigger_restarts_attack_from_zero() {
        let params = ModEnvParams {
            a_ms: 200.0,
            d_ms: 50.0,
            s: 0.8, // high sustain ⇒ big residual to retrigger from
            r_ms: 100.0,
            shape: AdsrShape::Lin,
        };

        // Time (in ticks) for the attack to first cross 0.5, from a fresh env.
        let ticks_to_half = |env: &mut ModEnvState| {
            let mut n = 0;
            while env.level < 0.5 && n < 100_000 {
                env.tick(DT);
                n += 1;
            }
            n
        };

        let mut env = ModEnvState::default();
        env.cook(&params);
        env.note_on();
        let first = ticks_to_half(&mut env);

        // Run it up to the sustain plateau.
        for _ in 0..5_000 {
            env.tick(DT);
        }
        assert!(env.level > 0.7, "did not reach sustain: {}", env.level);

        // Retrigger without going idle.
        env.note_on();
        assert_eq!(env.level, 0.0, "retrigger did not reset level to 0");

        let second = ticks_to_half(&mut env);
        // Same attack time on retrigger as on the first note (within a tick).
        assert!(
            (first as i64 - second as i64).abs() <= 1,
            "retrigger attack {second} ticks vs first {first} — not restarted from 0",
        );
    }
}
