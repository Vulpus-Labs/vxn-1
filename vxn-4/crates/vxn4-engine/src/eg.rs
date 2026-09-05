//! 4-rate / 4-level envelope generator, one per operator per voice.
//!
//! The brief allows for envelopes "extended past ADSR", and this is the
//! extension an FM synth wants: the DX shape, which vxn-2 already runs
//! (`vxn2_dsp::eg`). Stages are
//!
//! ```text
//! Idle → Attack (→L1) → Decay1 (→L2) → Decay2 (→L3) → Sustain → Release (→L4) → Idle
//! ```
//!
//! Each segment marches the level toward its target and terminates on arrival.
//! Level may rise *or* fall in any segment, so rising decays and rising
//! releases both work — which is most of why this beats ADSR for FM, where a
//! modulator that swells after the attack is a standard gesture.
//!
//! This is a deliberately smaller reimplementation than vxn-2's, not a port.
//! vxn-2's carries the DX7's quantised rate ladder, the level ladder from
//! ADR 0010, keyboard scaling and a log-domain marcher, because it is
//! reproducing specific hardware. vxn-4 is not, so rates here are plain
//! seconds-per-segment and levels are plain amplitudes. If vxn-4 later wants
//! DX rate/level fidelity, that is a port of `vxn2_dsp::eg`, not a change here.
//!
//! Ticked at **control rate** — once per block — not per sample.

/// Level below which a release is treated as finished.
pub const SILENCE: f32 = 1.0e-4;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Stage {
    #[default]
    Idle,
    Attack,
    Decay1,
    Decay2,
    Sustain,
    Release,
}

/// Envelope shape, as authored in a patch.
///
/// Times are seconds for the segment to traverse its whole span; levels are
/// linear amplitude in 0..=1.
#[derive(Clone, Copy, Debug)]
pub struct EgParams {
    /// Segment times in seconds: attack, decay1, decay2, release.
    pub t: [f32; 4],
    /// Segment targets in linear amplitude: L1 (attack peak), L2, L3
    /// (= sustain), L4 (release floor, normally 0).
    pub l: [f32; 4],
}

impl Default for EgParams {
    fn default() -> Self {
        Self {
            t: [0.005, 0.30, 1.0, 0.25],
            l: [1.0, 0.7, 0.5, 0.0],
        }
    }
}

impl EgParams {
    /// A plain ADSR, for patches that do not need the extra segment.
    ///
    /// Folds decay2 away by giving it a zero span (L2 == L3), so the marcher
    /// passes through it in one tick rather than needing a special case.
    pub fn adsr(a: f32, d: f32, s: f32, r: f32) -> Self {
        Self {
            t: [a, d, 0.0, r],
            l: [1.0, s, s, 0.0],
        }
    }

    /// A percussive shape: strike, fall to nothing, no sustain.
    pub fn perc(a: f32, d: f32) -> Self {
        Self {
            t: [a, d, 0.0, 0.010],
            l: [1.0, 0.0, 0.0, 0.0],
        }
    }
}

/// Running envelope state. One per operator per voice.
#[derive(Clone, Copy, Debug, Default)]
pub struct Eg {
    pub stage: Stage,
    pub level: f32,
    targets: [f32; 4],
    /// Amplitude per second for each segment, cooked from the patch times.
    rates: [f32; 4],
    /// Authored release time. Kept because the release *rate* cannot be cooked
    /// up front — see [`Eg::note_off`].
    release_secs: f32,
}

/// Amplitude-per-second to traverse `span` in `secs`.
///
/// A zero or negative time means "arrive next tick", expressed as a very large
/// rate rather than a branch in the marcher.
#[inline]
fn rate_for(span: f32, secs: f32) -> f32 {
    if secs <= 0.0 {
        1.0e9
    } else {
        (span.abs() / secs).max(1.0e-6)
    }
}

impl Eg {
    /// Resolve patch parameters into targets and march rates.
    ///
    /// `peak` scales every target — velocity lands here, so a soft note runs
    /// the same shape at a lower ceiling rather than a different shape.
    pub fn cook(&mut self, p: &EgParams, peak: f32) {
        self.targets = [
            p.l[0] * peak,
            p.l[1] * peak,
            p.l[2] * peak,
            p.l[3] * peak,
        ];
        // Each segment's span is measured from where the previous one ended, so
        // the authored time is the time that segment actually takes. The
        // release is the exception and is paced in `note_off` instead; the
        // value cooked here is only a fallback for a release entered without
        // one (nothing does that today).
        self.rates = [
            rate_for(self.targets[0], p.t[0]),
            rate_for(self.targets[1] - self.targets[0], p.t[1]),
            rate_for(self.targets[2] - self.targets[1], p.t[2]),
            rate_for(self.targets[3] - self.targets[2], p.t[3]),
        ];
        self.release_secs = p.t[3];
    }

    /// Begin the attack from the *current* level rather than from zero, so a
    /// retrigger on a still-sounding voice does not click.
    pub fn note_on(&mut self) {
        self.stage = Stage::Attack;
    }

    /// Enter the release segment, pacing it from the level the envelope is
    /// actually at.
    ///
    /// The rate cannot be cooked in [`Self::cook`] with the others. Those pace
    /// a segment across the span between two authored levels, which is correct
    /// because a segment always begins where the previous one ended. A release
    /// does not: it begins wherever the key was let go.
    ///
    /// Pacing it from the authored `L3 → L4` span instead is wrong in two ways,
    /// and the second is fatal. Releasing early — before sustain — would fade
    /// at the wrong speed. And for a percussive shape, where `L3` and `L4` are
    /// both zero, the span is zero, so the rate clamps to the `rate_for` floor
    /// and the envelope marches at 1e-6/sec: a note that never ends, holding
    /// its voice forever. That is what this used to do.
    pub fn note_off(&mut self) {
        if self.stage != Stage::Idle {
            self.stage = Stage::Release;
            self.rates[3] = rate_for(self.level - self.targets[3], self.release_secs);
        }
    }

    /// Force a fast linear release to silence over `secs`, from wherever the
    /// level currently is.
    ///
    /// This is the allocator's declick: every operator on a stolen voice gets
    /// the same wall-clock deadline, so they all arrive at zero together and
    /// the voice's spectrum collapses evenly instead of shedding operators one
    /// at a time (which sounds like a filter sweep, not a fade).
    pub fn kill(&mut self, secs: f32) {
        self.stage = Stage::Release;
        self.targets[3] = 0.0;
        self.rates[3] = rate_for(self.level, secs);
    }

    pub fn is_idle(&self) -> bool {
        self.stage == Stage::Idle
    }

    /// Hard reset to silence.
    pub fn clear(&mut self) {
        self.stage = Stage::Idle;
        self.level = 0.0;
    }

    /// Advance by `dt` seconds and return the new level.
    #[inline]
    pub fn tick(&mut self, dt: f32) -> f32 {
        let seg = match self.stage {
            Stage::Idle => {
                self.level = 0.0;
                return 0.0;
            }
            Stage::Sustain => return self.level,
            Stage::Attack => 0,
            Stage::Decay1 => 1,
            Stage::Decay2 => 2,
            Stage::Release => 3,
        };

        let target = self.targets[seg];
        let step = self.rates[seg] * dt;
        // One marcher for rising and falling segments both; `arrived` is the
        // only place the direction matters.
        let arrived = if self.level < target {
            self.level += step;
            self.level >= target
        } else {
            self.level -= step;
            self.level <= target
        };

        if arrived {
            self.level = target;
            self.stage = match self.stage {
                Stage::Attack => Stage::Decay1,
                Stage::Decay1 => Stage::Decay2,
                Stage::Decay2 => Stage::Sustain,
                _ => Stage::Idle,
            };
            // A sustain target at (or below) silence is a percussive patch that
            // has finished, not a note waiting for a key-up. Retiring it here is
            // what lets the allocator reclaim one-shot voices without a note-off.
            if self.stage == Stage::Sustain && self.level <= SILENCE {
                self.stage = Stage::Idle;
                self.level = 0.0;
            }
        }
        self.level
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f32 = 64.0 / 48_000.0;

    fn run(eg: &mut Eg, secs: f32) {
        for _ in 0..((secs / DT) as usize) {
            eg.tick(DT);
        }
    }

    #[test]
    fn adsr_reaches_sustain_and_holds() {
        let mut eg = Eg::default();
        eg.cook(&EgParams::adsr(0.01, 0.05, 0.6, 0.1), 1.0);
        eg.note_on();
        run(&mut eg, 0.5);
        assert_eq!(eg.stage, Stage::Sustain);
        assert!((eg.level - 0.6).abs() < 1e-3, "level {}", eg.level);
        run(&mut eg, 1.0);
        assert!((eg.level - 0.6).abs() < 1e-3, "sustain drifted to {}", eg.level);
    }

    #[test]
    fn release_returns_to_idle() {
        let mut eg = Eg::default();
        eg.cook(&EgParams::adsr(0.001, 0.01, 0.5, 0.05), 1.0);
        eg.note_on();
        run(&mut eg, 0.2);
        eg.note_off();
        run(&mut eg, 0.3);
        assert!(eg.is_idle());
        assert_eq!(eg.level, 0.0);
    }

    /// The extension past ADSR that matters for FM: a segment that rises.
    #[test]
    fn a_rising_decay_segment_swells() {
        let mut eg = Eg::default();
        eg.cook(
            &EgParams {
                t: [0.001, 0.05, 0.05, 0.05],
                l: [0.2, 0.9, 0.6, 0.0],
            },
            1.0,
        );
        eg.note_on();
        run(&mut eg, 0.005);
        let after_attack = eg.level;
        assert!((after_attack - 0.2).abs() < 0.05, "attack -> {after_attack}");
        run(&mut eg, 0.06);
        assert!(eg.level > 0.8, "decay1 should rise to 0.9, got {}", eg.level);
    }

    /// A percussive patch must retire itself with no note-off, or one-shot
    /// voices accumulate until the allocator starts stealing live notes.
    #[test]
    fn percussive_patch_retires_without_a_note_off() {
        let mut eg = Eg::default();
        eg.cook(&EgParams::perc(0.001, 0.05), 1.0);
        eg.note_on();
        run(&mut eg, 0.5);
        assert!(eg.is_idle(), "stage {:?} level {}", eg.stage, eg.level);
    }

    /// Releasing a percussive shape early must still end the note. `L3` and
    /// `L4` are both zero there, so a release paced from the authored span
    /// marches at the `rate_for` floor and the voice never retires — it holds
    /// its slot until the allocator steals it back.
    #[test]
    fn a_percussive_shape_released_early_still_reaches_silence() {
        let mut eg = Eg::default();
        eg.cook(&EgParams::perc(0.001, 0.55), 0.62);
        eg.note_on();
        run(&mut eg, 0.04); // still in decay1, well above zero
        assert!(eg.level > 0.3, "expected a live level, got {}", eg.level);
        eg.note_off();
        run(&mut eg, 0.05); // release time is 10 ms
        assert!(eg.is_idle(), "stuck at {:?} level {}", eg.stage, eg.level);
    }

    /// A release must take its authored time from wherever it starts, not from
    /// wherever the patch said sustain would be.
    #[test]
    fn release_is_paced_from_the_current_level() {
        let p = EgParams::adsr(0.001, 2.0, 0.9, 0.10);
        let mut early = Eg::default();
        early.cook(&p, 1.0);
        early.note_on();
        run(&mut early, 0.05); // released mid-decay, still near 1.0
        early.note_off();
        run(&mut early, 0.12);
        assert!(
            early.is_idle(),
            "early release overran: {:?} at {}",
            early.stage,
            early.level
        );
    }

    #[test]
    fn kill_reaches_silence_within_its_deadline() {
        let mut eg = Eg::default();
        eg.cook(&EgParams::adsr(0.001, 0.01, 0.9, 5.0), 1.0);
        eg.note_on();
        run(&mut eg, 0.2);
        assert!(eg.level > 0.5);
        eg.kill(0.005);
        run(&mut eg, 0.006);
        assert!(eg.is_idle(), "declick left {} at {:?}", eg.level, eg.stage);
    }

    /// Every operator killed at once must land together — a staggered collapse
    /// is audible as a timbre sweep rather than a fade.
    #[test]
    fn kill_lands_all_operators_together() {
        let mut egs = [Eg::default(); 4];
        for (i, eg) in egs.iter_mut().enumerate() {
            eg.cook(&EgParams::adsr(0.001, 0.01, 0.2 + 0.2 * i as f32, 5.0), 1.0);
            eg.note_on();
            for _ in 0..200 {
                eg.tick(DT);
            }
            eg.kill(0.005);
        }
        for _ in 0..((0.006 / DT) as usize) {
            for eg in egs.iter_mut() {
                eg.tick(DT);
            }
        }
        assert!(egs.iter().all(|e| e.is_idle()), "{:?}", egs.map(|e| e.level));
    }

    #[test]
    fn velocity_scales_the_ceiling_not_the_shape() {
        let p = EgParams::adsr(0.01, 0.05, 0.5, 0.1);
        let mut loud = Eg::default();
        let mut soft = Eg::default();
        loud.cook(&p, 1.0);
        soft.cook(&p, 0.25);
        loud.note_on();
        soft.note_on();
        run(&mut loud, 0.5);
        run(&mut soft, 0.5);
        assert_eq!(loud.stage, soft.stage, "shapes diverged");
        assert!((loud.level * 0.25 - soft.level).abs() < 1e-3);
    }
}
