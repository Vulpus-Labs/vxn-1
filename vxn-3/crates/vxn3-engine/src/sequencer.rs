//! The step sequencer — per-track step grid (ADR 0001 §2).
//!
//! 0047 is the basic case: a fixed 16-step grid of plain on/off trigs at a
//! 16th-note resolution, each step carrying a note + velocity. Per-track *length*
//! (polymeter), probability, and retrig n-over-m land in 0048 — `Pattern::len`
//! is already per-track so polymeter is a small extension, not a rewrite.
//!
//! The grid is *position*, not time: the instrument engine maps the host beat
//! clock onto step indices and schedules trigs sample-accurately (see
//! [`crate::engine`]). The sequencer here is pure data.

/// Steps per quarter-note beat. 16th-note grid.
pub const STEPS_PER_BEAT: f64 = 4.0;
/// Length of one step in beats.
pub const STEP_BEATS: f64 = 1.0 / STEPS_PER_BEAT;
/// Maximum steps in a pattern (the storage ceiling; `len` may be shorter).
pub const MAX_STEPS: usize = 16;

/// One step. `active` gates the trig; `note`/`velocity` shape it.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Step {
    pub active: bool,
    /// Equal-tempered MIDI note (fractional allowed).
    pub note: f32,
    /// 0..1.
    pub velocity: f32,
}

impl Default for Step {
    fn default() -> Self {
        Self {
            active: false,
            note: 36.0, // C2
            velocity: 1.0,
        }
    }
}

/// A track's step grid.
#[derive(Copy, Clone, Debug)]
pub struct Pattern {
    pub steps: [Step; MAX_STEPS],
    /// Active length in steps (≤ [`MAX_STEPS`]). Per-track → polymeter in 0048.
    pub len: usize,
}

impl Default for Pattern {
    fn default() -> Self {
        Self {
            steps: [Step::default(); MAX_STEPS],
            len: MAX_STEPS,
        }
    }
}

impl Pattern {
    /// Enable a step with the given note/velocity (no-op if out of range).
    pub fn set(&mut self, index: usize, note: f32, velocity: f32) {
        if index < MAX_STEPS {
            self.steps[index] = Step {
                active: true,
                note,
                velocity,
            };
        }
    }

    /// Clear a step.
    pub fn clear(&mut self, index: usize) {
        if index < MAX_STEPS {
            self.steps[index].active = false;
        }
    }

    /// The step at a global 16th index, wrapped into this pattern's length.
    #[inline]
    pub fn step_at(&self, global_index: i64) -> Step {
        let len = self.len.max(1) as i64;
        self.steps[global_index.rem_euclid(len) as usize]
    }
}
