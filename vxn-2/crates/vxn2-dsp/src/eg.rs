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
    pub level: f32,
    pub targets: [f32; 4],
    pub rates_per_sec: [f32; 4],
}

/// Prototype toggle: `true` = DX7-faithful **logarithmic** level curve
/// (`amp = 2^((L-99)/8)`, ~6 dB per 8 steps); `false` = legacy perceptual
/// square (`(L/99)^2`). Flip + rebuild for a clean A/B. Applies to both the EG
/// L-values and the operator output level (see `op.rs`/`stack.rs` cook), which
/// is why moderate-level modulators were ~30× too hot under the square curve.
pub const EG_LOG_LEVELS: bool = true;

/// Convert a DX7-style level (0..99) to a normalised amplitude in [0, 1].
#[inline]
pub fn level_to_amp(level: u8) -> f32 {
    if level == 0 {
        return 0.0;
    }
    if EG_LOG_LEVELS {
        // DX7 log curve: 0 dB at L=99, −6 dB per 8 steps (≈ −74 dB at L=1).
        2_f32.powf((level.min(99) as f32 - 99.0) / 8.0)
    } else {
        let l = level.min(99) as f32 / 99.0;
        l * l
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

impl EgState {
    /// Bake `params` into runtime increments + targets, scaled by `max_amp`
    /// (the cooked per-note ceiling: level × ks × vel) and `rate_mult` (the
    /// key-rate scaling factor — see [`crate::ks::ks_rate_mult`]).
    pub fn cook(&mut self, params: &EgParams, max_amp: f32, rate_mult: f32) {
        for i in 0..4 {
            self.targets[i] = level_to_amp(params.l[i]) * max_amp;
            self.rates_per_sec[i] = rate_to_amp_per_sec(params.r[i]) * rate_mult;
        }
    }

    /// Trigger the attack stage. Level continues from wherever it is — this
    /// supports retrigger without click.
    pub fn note_on(&mut self) {
        self.stage = EgStage::Attack;
    }

    /// Move to release. From any stage except Idle.
    pub fn note_off(&mut self) {
        if self.stage != EgStage::Idle {
            self.stage = EgStage::Release;
        }
    }

    /// Advance one control tick, `dt` seconds since the previous tick.
    /// Returns the post-tick level.
    pub fn tick(&mut self, dt: f32) -> f32 {
        match self.stage {
            EgStage::Idle => {
                self.level = self.targets[3];
            }
            EgStage::Attack => self.march(0, EgStage::Decay1, dt),
            EgStage::Decay1 => self.march(1, EgStage::Decay2, dt),
            EgStage::Decay2 => self.march(2, EgStage::Sustain, dt),
            EgStage::Sustain => { /* hold at L3 */ }
            EgStage::Release => self.march(3, EgStage::Idle, dt),
        }
        self.level
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

    fn default_params() -> EgParams {
        EgParams {
            r: [99, 50, 35, 60],
            l: [99, 70, 50, 0],
        }
    }

    #[test]
    fn level_to_amp_endpoints() {
        assert!((level_to_amp(0)).abs() < 1e-6);
        assert!((level_to_amp(99) - 1.0).abs() < 1e-6);
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
        eg.cook(&default_params(), 1.0, 1.0);
        eg.note_on();
        let dt = 1.0 / 48_000.0;
        let mut reached_attack_top = false;
        let mut reached_sustain = false;
        for _ in 0..(48_000 * 2) {
            eg.tick(dt);
            if eg.stage == EgStage::Decay1 {
                reached_attack_top = true;
            }
            if eg.stage == EgStage::Sustain {
                reached_sustain = true;
                break;
            }
        }
        assert!(reached_attack_top, "never finished attack");
        assert!(reached_sustain, "never reached sustain");
        // Sustain target = L3=50 through the active level curve.
        let want = level_to_amp(50);
        assert!(
            (eg.level - want).abs() < 0.01,
            "sustain level off: {} (want {want})",
            eg.level
        );
    }

    #[test]
    fn release_drops_to_l4() {
        let mut eg = EgState::default();
        eg.cook(&default_params(), 1.0, 1.0);
        eg.note_on();
        let dt = 1.0 / 48_000.0;
        for _ in 0..(48_000 * 2) {
            eg.tick(dt);
            if eg.stage == EgStage::Sustain {
                break;
            }
        }
        eg.note_off();
        for _ in 0..(48_000 * 5) {
            eg.tick(dt);
            if eg.stage == EgStage::Idle {
                break;
            }
        }
        assert_eq!(eg.stage, EgStage::Idle);
        assert!((eg.level - 0.0).abs() < 1e-3);
    }

    #[test]
    fn rate_mult_speeds_attack() {
        let params = default_params();
        let mut a = EgState::default();
        a.cook(&params, 1.0, 1.0);
        a.note_on();
        let mut b = EgState::default();
        b.cook(&params, 1.0, 4.0);
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
