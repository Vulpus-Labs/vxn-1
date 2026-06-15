//! The step sequencer data model (ADR 0001 §2).
//!
//! A pattern is **one track's** independent lane: its own step count *and* its
//! own lane-local tick (step duration in beats), so tracks with different
//! lengths/divisors phase against each other — polymeter "for free". The grid is
//! pure data; the stateful per-block resolution (phase, probability, retrig,
//! transport-jump resync) lives in [`crate::lane`].
//!
//! Trig **attributes** (probability, retrig n/m/curve/velocity ramp) live on the
//! step — they have no base to revert to. Continuous params that *do* have a
//! base get p-locked in 0050; that split is deliberate (ADR 0001 §3a).

/// One step in a 16th-note's worth of a beat (a common lane-local tick).
pub const SIXTEENTH: f64 = 0.25;
/// One step = one straight 8th note.
pub const EIGHTH: f64 = 0.5;
/// One step = one 8th-note triplet (3 per beat → triplet feel).
pub const EIGHTH_TRIPLET: f64 = 1.0 / 3.0;
/// Maximum steps in a pattern (storage ceiling; `len` may be shorter).
pub const MAX_STEPS: usize = 16;

/// Retrig timing curve — how the `n` hits are spaced across the window.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum RetrigCurve {
    /// Evenly spaced.
    #[default]
    Even,
    /// Speeding up — gaps shrink over the window (a roll into the next hit).
    Accel,
    /// Slowing down — gaps grow over the window.
    Decel,
}

impl RetrigCurve {
    /// Map a normalised index `u = j/n ∈ [0, 1)` to a normalised position in the
    /// window `[0, 1)`. `pos(0) = 0` always (first hit at the window start).
    #[inline]
    pub fn position(self, u: f64) -> f64 {
        match self {
            RetrigCurve::Even => u,
            RetrigCurve::Accel => u.sqrt(), // gaps shrink
            RetrigCurve::Decel => u * u,    // gaps grow
        }
    }
}

/// Retrig macro on a trig: fire `n` hits across `m` steps (ADR 0001 §2).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Retrig {
    /// Hit count. `1` (or `0`) = no retrig, a single hit.
    pub n: u8,
    /// Window span in lane steps.
    pub m: u8,
    /// Timing curve across the window.
    pub curve: RetrigCurve,
    /// Velocity at the last hit (the first uses the step's velocity); a ramp.
    pub vel_end: f32,
}

impl Default for Retrig {
    fn default() -> Self {
        Self {
            n: 1,
            m: 1,
            curve: RetrigCurve::Even,
            vel_end: 1.0,
        }
    }
}

impl Retrig {
    #[inline]
    pub fn is_retrig(&self) -> bool {
        self.n >= 2 && self.m >= 1
    }
}

/// One step: a gated trig plus its attributes.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Step {
    pub active: bool,
    /// Equal-tempered MIDI note (fractional allowed).
    pub note: f32,
    /// 0..1.
    pub velocity: f32,
    /// Fire probability per pass: `>= 1.0` always, `<= 0.0` never.
    pub probability: f32,
    pub retrig: Retrig,
}

impl Default for Step {
    fn default() -> Self {
        Self {
            active: false,
            note: 36.0, // C2
            velocity: 1.0,
            probability: 1.0,
            retrig: Retrig::default(),
        }
    }
}

/// A continuous track/engine parameter a p-lock can override (ADR 0001 §3a).
/// Trig attributes (probability, retrig, velocity) are *not* here — they live on
/// the trig (0048). The send amount joins once 0051 lands.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LockParam {
    Gain,
    Pan,
    Decay,
    Tone,
    Pitch,
    /// Delay send amount — p-locking this high on a step is the dub throw (0051).
    Send,
}

/// Number of lockable params; the lock table and resolver are sized to it.
pub const N_LOCK_PARAMS: usize = 6;

impl LockParam {
    #[inline]
    pub fn index(self) -> usize {
        match self {
            LockParam::Gain => 0,
            LockParam::Pan => 1,
            LockParam::Decay => 2,
            LockParam::Tone => 3,
            LockParam::Pitch => 4,
            LockParam::Send => 5,
        }
    }
}

/// How a p-lock ends (the step-shape subset — no ramp yet).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Termination {
    /// Hold for `n` lane-local ticks, then release back to base. `n = 1` is the
    /// momentary spike.
    Revert { n: u16 },
    /// Hold until the next lock on this param; persists across the loop wrap.
    Latch,
}

/// A per-step parameter lock (step shape).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Lock {
    pub value: f32,
    pub termination: Termination,
}

/// A track's lane: step grid + length + lane-local tick + p-lock table.
#[derive(Copy, Clone, Debug)]
pub struct Pattern {
    pub steps: [Step; MAX_STEPS],
    /// Active length in steps (≤ [`MAX_STEPS`]). Different per track → polymeter.
    pub len: usize,
    /// Lane-local tick: duration of one step in quarter-note beats. Different per
    /// track → tracks run at different rates and phase.
    pub step_beats: f64,
    /// Sparse `(step, param) → lock` table. `None` = no lock at that cell.
    pub locks: [[Option<Lock>; N_LOCK_PARAMS]; MAX_STEPS],
}

impl Default for Pattern {
    fn default() -> Self {
        Self {
            steps: [Step::default(); MAX_STEPS],
            len: MAX_STEPS,
            step_beats: SIXTEENTH,
            locks: [[None; N_LOCK_PARAMS]; MAX_STEPS],
        }
    }
}

impl Pattern {
    /// Enable a step with note/velocity (probability 1.0, no retrig).
    pub fn set(&mut self, index: usize, note: f32, velocity: f32) {
        if index < MAX_STEPS {
            let s = &mut self.steps[index];
            s.active = true;
            s.note = note;
            s.velocity = velocity;
        }
    }

    /// Set a step's fire probability (enables it).
    pub fn set_probability(&mut self, index: usize, probability: f32) {
        if index < MAX_STEPS {
            self.steps[index].active = true;
            self.steps[index].probability = probability;
        }
    }

    /// Set a step's retrig macro (enables it).
    pub fn set_retrig(&mut self, index: usize, retrig: Retrig) {
        if index < MAX_STEPS {
            self.steps[index].active = true;
            self.steps[index].retrig = retrig;
        }
    }

    /// Clear a step.
    pub fn clear(&mut self, index: usize) {
        if index < MAX_STEPS {
            self.steps[index].active = false;
        }
    }

    /// The step at a global lane index, wrapped into this pattern's length.
    /// `len` is clamped to `[1, MAX_STEPS]` so an out-of-range `len` can't index
    /// past the storage.
    #[inline]
    pub fn step_at(&self, global_index: i64) -> Step {
        let len = self.len.clamp(1, MAX_STEPS) as i64;
        self.steps[global_index.rem_euclid(len) as usize]
    }

    /// Set a p-lock on a (step, param) cell.
    pub fn set_lock(&mut self, step: usize, param: LockParam, lock: Lock) {
        if step < MAX_STEPS {
            self.locks[step][param.index()] = Some(lock);
        }
    }

    /// Clear a p-lock cell.
    pub fn clear_lock(&mut self, step: usize, param: LockParam) {
        if step < MAX_STEPS {
            self.locks[step][param.index()] = None;
        }
    }

    /// The lock (if any) at a global lane index for `param_index`.
    #[inline]
    pub fn lock_at(&self, global_index: i64, param_index: usize) -> Option<Lock> {
        let len = self.len.clamp(1, MAX_STEPS) as i64;
        self.locks[global_index.rem_euclid(len) as usize][param_index]
    }
}
