//! 4-rate / 4-level envelope generator, DX7-shape approximation.
//!
//! Stages: `Idle → Attack (→L1) → Decay1 (→L2) → Decay2 (→L3) → Sustain
//! → Release (→L4) → Idle`. Each segment marches the current level toward
//! its target at a rate-derived increment; a target reached terminates the
//! segment. Level may be increasing or decreasing in any segment (DX7
//! supports rising decays and rising releases).
//!
//! Fidelity: levels (0..99) → amplitude via a perceptual square curve,
//! `amp = (L/99)^2`. Rates (0..99) → log-spaced amp-per-second between
//! ~0.05/s (R=0, ~20 s sweep) and ~250/s (R=99, ~4 ms sweep). Matches DX7
//! shape, not byte-exact (per ticket: "approximate DX7 shape").
//!
//! Tick rate: the EG advances per *control sample* — typically once per
//! audio block, or every M samples for sub-block envelopes. The caller
//! passes `dt` in seconds.

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct EgParams {
    pub r: [u8; 4],
    pub l: [u8; 4],
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum EgStage {
    #[default]
    Idle,
    Attack,
    Decay1,
    Decay2,
    Sustain,
    Release,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct EgState {
    pub stage: EgStage,
    /// Output amplitude — the value the per-sample lane loop reads. Always in
    /// linear amplitude regardless of curve.
    pub level: f32,
    /// Per-segment amplitude targets (`level_to_amp(L) × max_amp`). Used by the
    /// `Lin` marcher and the `Exp` attack (both march `level` in amplitude);
    /// also the reference the lane loop / tests inspect.
    pub targets: [f32; 4],
    /// Per-segment amplitude march rates (amp/sec) — the `Lin` path and the
    /// declick ([`kill_release`](Self::kill_release)).
    pub rates_per_sec: [f32; 4],
    /// Curve selected at cook (ticket 0125). `Exp` marches the **downward**
    /// segments (Decay1/Decay2/Release) in the log2 domain — linear-in-dB →
    /// exponential amplitude taper, the characteristic DX7 envelope. Attack
    /// stays a linear-amplitude rise (DX7 attack is fast/punchy, not a
    /// dead-quiet log creep). `Lin` marches every segment in amplitude.
    curve: EgCurve,
    /// Linear amplitude ceiling (`OL × ks × vel`) applied after `exp2` on the
    /// `Exp` downward path. `targets` already folds it in; kept separately so
    /// the log-domain output `max_amp × 2^log_level` matches `targets`.
    max_amp: f32,
    /// `Exp` downward-segment marcher position, in log2 units relative to
    /// `max_amp` (0 → full `max_amp`, [`EG_LOG_FLOOR`] → silent).
    log_level: f32,
    /// `Exp` log2 targets per segment (`(L-99)/8`, or [`EG_LOG_FLOOR`] for L=0).
    log_targets: [f32; 4],
    /// `Exp` log2 march rates (log2 units/sec).
    log_rates: [f32; 4],
    /// Declick override: a [`kill_release`](Self::kill_release) marches `level`
    /// linearly to 0 in the amplitude domain on **both** curves (a fast smooth
    /// ramp, no exponential tail needed), so the Release stage ignores the log
    /// marcher while set. Cleared on `note_on` / `note_off`.
    kill: bool,
}

/// Log2 floor for the `Exp` downward marcher (≈ −90 dB). A segment targeting a
/// zero level (`L=0`) marches toward this floor, then snaps to true silence on
/// the stage transition — `2^EG_LOG_FLOOR` is inaudible, and marching to a
/// finite floor keeps the dB/sec rate well-defined (true 0 is `-inf` in log2).
const EG_LOG_FLOOR: f32 = -15.0;

/// Per-operator level→amplitude curve (ticket 0124). Selects how a DX7-style
/// level (0..99) — for both the EG L-values and the operator output level —
/// maps to a normalised amplitude. Patch state, default [`EgCurve::Exp`]; the
/// choice is made in `cook` (control rate, scalar) so the per-sample lane loop
/// is untouched (see [`level_to_amp`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum EgCurve {
    /// DX7-faithful **logarithmic** curve (`amp = 2^((L-99)/8)`, ~6 dB per 8
    /// steps). The corrected default — moderate-level modulators were ~30× too
    /// hot under the legacy square curve. See ADR 0007.
    #[default]
    Exp = 0,
    /// Legacy perceptual **square** curve (`(L/99)^2`). Escape hatch preserving
    /// the pre-0123 behaviour on a per-op basis.
    Lin = 1,
}

/// Convert a DX7-style level (0..99) to a normalised amplitude in [0, 1] under
/// the given per-op [`EgCurve`]. `L=0` is always hard silence.
#[inline]
pub fn level_to_amp(level: u8, curve: EgCurve) -> f32 {
    if level == 0 {
        return 0.0;
    }
    match curve {
        // DX7 log curve: 0 dB at L=99, −6 dB per 8 steps (≈ −74 dB at L=1).
        EgCurve::Exp => 2_f32.powf((level.min(99) as f32 - 99.0) / 8.0),
        EgCurve::Lin => {
            let l = level.min(99) as f32 / 99.0;
            l * l
        }
    }
}

/// Convert a DX7-style rate (0..99) to amplitude-per-second.
///
/// R=0 ≈ 0.05/s (~20s sweep); R=99 ≈ ~250/s (~4ms sweep). Log-spaced.
#[inline]
pub fn rate_to_amp_per_sec(rate: u8) -> f32 {
    let r = rate.min(99) as f32;
    0.05 * (2_f32).powf(r * 0.125)
}

/// Full log2 span of an `Exp` envelope segment: floor (silence) → 0 dB.
const EG_LOG_SPAN: f32 = -EG_LOG_FLOOR;

/// Convert a DX7-style rate (0..99) to **log2 units per second** for the `Exp`
/// downward marcher (ticket 0125). Calibrated so a full-span sweep (silence →
/// full) takes ≈ 20 s at R=0 and ≈ 4 ms at R=99 — the same log-spacing as
/// [`rate_to_amp_per_sec`], rescaled to the log2 span. Because the march is
/// linear-in-dB at a constant rate, a *partial* segment (a small dB step) is
/// proportionally quicker, exactly as on a DX7.
#[inline]
pub fn rate_to_log2_per_sec(rate: u8) -> f32 {
    let r = rate.min(99) as f32;
    // R=0 → EG_LOG_SPAN / 20 s; ×2 per 8 rate steps (same slope as the amp rate).
    (EG_LOG_SPAN / 20.0) * (2_f32).powf(r * 0.125)
}

/// log2 of a linear amplitude relative to `max_amp`, floored at [`EG_LOG_FLOOR`].
/// Used to seed the `Exp` log marcher from the current amplitude `level` when a
/// downward segment begins from an amplitude-domain state (attack, sustain hold,
/// retrigger).
#[inline]
fn amp_to_log(level: f32, max_amp: f32) -> f32 {
    if level <= 0.0 || max_amp <= 0.0 {
        return EG_LOG_FLOOR;
    }
    (level / max_amp).log2().max(EG_LOG_FLOOR)
}

impl EgState {
    /// Bake `params` into runtime increments + targets, scaled by `max_amp`
    /// (the cooked per-note ceiling: level × ks × vel) and `rate_mult` (the
    /// key-rate scaling factor — see [`crate::ks::ks_rate_mult`]). `curve`
    /// selects the per-op level→amplitude mapping for the L-targets.
    pub fn cook(&mut self, params: &EgParams, max_amp: f32, rate_mult: f32, curve: EgCurve) {
        self.curve = curve;
        self.max_amp = max_amp;
        for i in 0..4 {
            self.targets[i] = level_to_amp(params.l[i], curve) * max_amp;
            self.rates_per_sec[i] = rate_to_amp_per_sec(params.r[i]) * rate_mult;
            // Exp downward-marcher state: log2 target (normalised to max_amp) and
            // a log2/sec rate. `(L-99)/8` is exactly `log2(level_to_amp(L, Exp))`;
            // L=0 → the silence floor.
            self.log_targets[i] = if params.l[i] == 0 {
                EG_LOG_FLOOR
            } else {
                (params.l[i].min(99) as f32 - 99.0) / 8.0
            };
            self.log_rates[i] = rate_to_log2_per_sec(params.r[i]) * rate_mult;
        }
    }

    /// Trigger the attack stage. Level continues from wherever it is — this
    /// supports retrigger without click.
    pub fn note_on(&mut self) {
        self.kill = false;
        self.stage = EgStage::Attack;
    }

    /// Move to release. From any stage except Idle. On the `Exp` curve, seed the
    /// log marcher from the current amplitude so the exponential release begins
    /// from wherever the envelope currently sits.
    pub fn note_off(&mut self) {
        if self.stage != EgStage::Idle {
            self.kill = false;
            self.log_level = amp_to_log(self.level, self.max_amp);
            self.stage = EgStage::Release;
        }
    }

    /// Force a fast release to 0 over `secs`, overriding the patch's release
    /// target/rate — used to declick a killed voice. A linear amplitude ramp
    /// (`rate = level / secs`) on both curves: the declick only needs to be
    /// smooth and fast, not exponentially shaped, and a linear ramp reaches 0 in
    /// exactly `secs` from the current level. Already-silent EGs go straight to
    /// Idle.
    pub fn kill_release(&mut self, secs: f32) {
        if self.level <= 0.0 {
            self.stage = EgStage::Idle;
            return;
        }
        self.kill = true;
        self.targets[3] = 0.0;
        self.rates_per_sec[3] = self.level / secs.max(1.0e-6);
        self.stage = EgStage::Release;
    }

    /// Advance one control tick, `dt` seconds since the previous tick.
    /// Returns the post-tick level.
    ///
    /// `Lin` marches every segment linearly in amplitude. `Exp` marches the
    /// downward segments (Decay1/Decay2/Release) linearly in log2 (→ exponential
    /// amplitude taper), but keeps a linear-amplitude attack and an
    /// amplitude-domain declick. The marcher is scalar, run once per control
    /// tick — never in the per-sample lane loop.
    pub fn tick(&mut self, dt: f32) -> f32 {
        let log = self.curve == EgCurve::Exp;
        match self.stage {
            EgStage::Idle => {
                self.level = self.targets[3];
            }
            // Attack is always a linear-amplitude rise. On Exp, seed the log
            // marcher from the reached top so Decay1 continues smoothly.
            EgStage::Attack => {
                self.march(0, EgStage::Decay1, dt);
                if log && self.stage == EgStage::Decay1 {
                    self.log_level = amp_to_log(self.level, self.max_amp);
                }
            }
            EgStage::Decay1 if log => self.march_log(1, EgStage::Decay2, dt),
            EgStage::Decay2 if log => self.march_log(2, EgStage::Sustain, dt),
            EgStage::Release if log && !self.kill => self.march_log(3, EgStage::Idle, dt),
            EgStage::Decay1 => self.march(1, EgStage::Decay2, dt),
            EgStage::Decay2 => self.march(2, EgStage::Sustain, dt),
            EgStage::Sustain => { /* hold at L3 */ }
            EgStage::Release => self.march(3, EgStage::Idle, dt),
        }
        self.level
    }

    /// March `log_level` linearly toward `log_targets[idx]` at `log_rates[idx]`
    /// (the `Exp` downward path), then project to amplitude
    /// `max_amp × 2^log_level`. On reaching the target, snap to the matching
    /// amplitude (true 0 when the segment targets silence) and advance to `next`.
    #[inline]
    fn march_log(&mut self, idx: usize, next: EgStage, dt: f32) {
        let target = self.log_targets[idx];
        let step = self.log_rates[idx] * dt;
        let mut reached = false;
        if self.log_level < target {
            self.log_level += step;
            if self.log_level >= target {
                self.log_level = target;
                reached = true;
            }
        } else if self.log_level > target {
            self.log_level -= step;
            if self.log_level <= target {
                self.log_level = target;
                reached = true;
            }
        } else {
            reached = true;
        }
        // Project log → amplitude. At the silence floor, output true 0.
        self.level = if self.log_level <= EG_LOG_FLOOR {
            0.0
        } else {
            self.max_amp * self.log_level.exp2()
        };
        if reached {
            self.level = self.targets[idx];
            self.stage = next;
        }
    }

    /// March `level` toward `targets[idx]` at `rates_per_sec[idx]`. Transition
    /// to `next` when the target is reached.
    #[inline]
    fn march(&mut self, idx: usize, next: EgStage, dt: f32) {
        let target = self.targets[idx];
        let step = self.rates_per_sec[idx] * dt;
        if self.level < target {
            self.level += step;
            if self.level >= target {
                self.level = target;
                self.stage = next;
            }
        } else if self.level > target {
            self.level -= step;
            if self.level <= target {
                self.level = target;
                self.stage = next;
            }
        } else {
            self.stage = next;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util;

    fn default_params() -> EgParams {
        EgParams {
            r: [99, 50, 35, 60],
            l: [99, 70, 50, 0],
        }
    }

    #[test]
    fn level_to_amp_endpoints() {
        for curve in [EgCurve::Exp, EgCurve::Lin] {
            assert!((level_to_amp(0, curve)).abs() < 1e-6);
            assert!((level_to_amp(99, curve) - 1.0).abs() < 1e-6);
        }
    }

    /// Run an EG to sustain, then sample the decay-to-L3 amplitude trajectory.
    fn decay_samples(curve: EgCurve) -> Vec<f32> {
        // Slow, long decay (L1=99 → L3=20) so the Decay segments occupy many
        // ticks and the shape is well-resolved.
        let params = EgParams { r: [99, 20, 20, 60], l: [99, 99, 20, 0] };
        let mut eg = EgState::default();
        eg.cook(&params, 1.0, 1.0, curve);
        eg.note_on();
        let dt = 1.0 / 1_000.0; // 1 kHz control rate
        let mut out = Vec::new();
        for _ in 0..20_000 {
            eg.tick(dt);
            if eg.stage == EgStage::Decay2 || eg.stage == EgStage::Sustain {
                out.push(eg.level);
            }
            if eg.stage == EgStage::Sustain {
                break;
            }
        }
        out
    }

    #[test]
    fn exp_decay_is_linear_in_db() {
        // The Exp marcher steps a constant amount in log2 per tick, so the dB
        // drop between successive samples is ~constant — the DX7 exponential
        // taper. (A linear-amplitude decay would have a *growing* dB step.)
        let s = decay_samples(EgCurve::Exp);
        assert!(s.len() > 50, "decay too short to measure: {}", s.len());
        let mid = s.len() / 2;
        let d_early = (s[10] / s[20]).log2();
        let d_late = (s[mid] / s[mid + 10]).log2();
        // Constant log slope → the two dB steps match within 10%.
        assert!(
            (d_early - d_late).abs() < 0.1 * d_early.abs().max(1e-6),
            "Exp decay not linear-in-dB: early {d_early}, late {d_late}"
        );
    }

    #[test]
    fn lin_decay_is_linear_in_amp() {
        // The Lin marcher steps a constant amount in amplitude per tick, so the
        // amplitude drop between successive samples is ~constant.
        let s = decay_samples(EgCurve::Lin);
        assert!(s.len() > 50, "decay too short: {}", s.len());
        let mid = s.len() / 2;
        let d_early = s[10] - s[20];
        let d_late = s[mid] - s[mid + 10];
        assert!(
            (d_early - d_late).abs() < 0.1 * d_early.abs().max(1e-6),
            "Lin decay not linear-in-amp: early {d_early}, late {d_late}"
        );
    }

    #[test]
    fn exp_rate_zero_is_far_slower_than_max() {
        // Sanity on the recalibrated log rate: a full release at R=0 takes many
        // seconds; at R=99 it's near-instant.
        let secs_to_idle = |r4: u8| {
            let params = EgParams { r: [99, 99, 99, r4], l: [99, 99, 99, 0] };
            let mut eg = EgState::default();
            eg.cook(&params, 1.0, 1.0, EgCurve::Exp);
            eg.note_on();
            let dt = 1.0 / 1_000.0;
            test_util::run_until_stage(|| { eg.tick(dt); eg.stage == EgStage::Sustain }, 2000);
            eg.note_off();
            let mut n = 0u32;
            test_util::run_until_stage(
                || { eg.tick(dt); n += 1; eg.stage == EgStage::Idle },
                1_000_000,
            );
            n as f32 * dt
        };
        let slow = secs_to_idle(0);
        let fast = secs_to_idle(99);
        assert!(slow > 5.0, "R=0 release should be slow, got {slow}s");
        assert!(fast < 0.05, "R=99 release should be near-instant, got {fast}s");
    }

    #[test]
    fn kill_release_declicks_linearly_on_exp() {
        // The declick is a linear amplitude ramp to 0 on both curves.
        let mut eg = EgState::default();
        eg.cook(&default_params(), 1.0, 1.0, EgCurve::Exp);
        eg.note_on();
        eg.stage = EgStage::Sustain;
        eg.level = 0.5;
        eg.kill_release(0.01); // 10 ms
        let dt = 1.0 / 48_000.0;
        let mut prev = eg.level;
        let mut steps = Vec::new();
        for _ in 0..1000 {
            eg.tick(dt);
            steps.push(prev - eg.level);
            prev = eg.level;
            if eg.stage == EgStage::Idle {
                break;
            }
        }
        assert_eq!(eg.stage, EgStage::Idle);
        assert!(eg.level.abs() < 1e-6);
        // Constant amplitude step until it hits 0 → linear declick.
        let s0 = steps[0];
        assert!(s0 > 0.0);
        assert!(
            steps.iter().take(5).all(|s| (s - s0).abs() < 1e-6),
            "declick not linear: {steps:?}"
        );
    }

    #[test]
    fn exp_curve_is_log_lin_is_square() {
        // L=50 under the DX7 log curve ≈ −37 dB (ADR 0007); under the legacy
        // square curve ≈ (50/99)^2 ≈ 0.255. The log value is ~15× quieter —
        // the ~30× hot-modulator gap the curve change closed.
        let exp = level_to_amp(50, EgCurve::Exp);
        let lin = level_to_amp(50, EgCurve::Lin);
        assert!((exp - 2_f32.powf((50.0 - 99.0) / 8.0)).abs() < 1e-6);
        assert!((lin - (50.0_f32 / 99.0).powi(2)).abs() < 1e-6);
        assert!(exp < lin * 0.1, "log curve should be far quieter at L=50");
    }

    #[test]
    fn rate_log_spaced() {
        let r0 = rate_to_amp_per_sec(0);
        let r50 = rate_to_amp_per_sec(50);
        let r99 = rate_to_amp_per_sec(99);
        assert!(r0 < r50 && r50 < r99);
        assert!(r0 < 0.1);
        assert!(r99 > 100.0);
    }

    #[test]
    fn attack_then_decay_then_sustain() {
        let mut eg = EgState::default();
        eg.cook(&default_params(), 1.0, 1.0, EgCurve::Exp);
        eg.note_on();
        let dt = 1.0 / 48_000.0;
        let mut reached_attack_top = false;
        let reached_sustain = test_util::run_until_stage(
            || {
                eg.tick(dt);
                if eg.stage == EgStage::Decay1 {
                    reached_attack_top = true;
                }
                eg.stage == EgStage::Sustain
            },
            48_000 * 2,
        );
        assert!(reached_attack_top, "never finished attack");
        assert!(reached_sustain, "never reached sustain");
        // Sustain target = L3=50 through the active level curve.
        let want = level_to_amp(50, EgCurve::Exp);
        assert!(
            (eg.level - want).abs() < 0.01,
            "sustain level off: {} (want {want})",
            eg.level
        );
    }

    #[test]
    fn release_drops_to_l4() {
        let mut eg = EgState::default();
        eg.cook(&default_params(), 1.0, 1.0, EgCurve::Exp);
        eg.note_on();
        let dt = 1.0 / 48_000.0;
        test_util::run_until_stage(|| { eg.tick(dt); eg.stage == EgStage::Sustain }, 48_000 * 2);
        eg.note_off();
        test_util::run_until_stage(|| { eg.tick(dt); eg.stage == EgStage::Idle }, 48_000 * 5);
        assert_eq!(eg.stage, EgStage::Idle);
        assert!((eg.level - 0.0).abs() < 1e-3);
    }

    #[test]
    fn rate_mult_speeds_attack() {
        let params = default_params();
        let mut a = EgState::default();
        a.cook(&params, 1.0, 1.0, EgCurve::Exp);
        a.note_on();
        let mut b = EgState::default();
        b.cook(&params, 1.0, 4.0, EgCurve::Exp);
        b.note_on();
        let dt = 1.0 / 48_000.0;
        let mut ticks_a = 0;
        while a.stage == EgStage::Attack {
            a.tick(dt);
            ticks_a += 1;
            if ticks_a > 480_000 {
                break;
            }
        }
        let mut ticks_b = 0;
        while b.stage == EgStage::Attack {
            b.tick(dt);
            ticks_b += 1;
            if ticks_b > 480_000 {
                break;
            }
        }
        assert!(ticks_a > 2 * ticks_b, "4× rate didn't shorten attack");
    }
}
