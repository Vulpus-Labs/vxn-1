//! A track: one active engine + its pattern + mix settings + swap mailbox
//! (ADR 0001 §4). Eight of these make the instrument.

use std::sync::Arc;

use crate::engines::KickTone;
use crate::lane::{Hit, LaneState};
use crate::sequencer::{LockParam, N_LOCK_PARAMS, Pattern};
use crate::swap::EngineSwap;
use crate::track_engine::{Knob, TrackEngine};

pub struct Track {
    /// The single active engine. Swapped off-thread via [`Track::swap`].
    pub engine: Box<dyn TrackEngine>,
    /// Main↔audio swap mailbox; clone the `Arc` to drive swaps from the main
    /// thread.
    pub swap: Arc<EngineSwap>,
    /// Step grid + p-lock table.
    pub pattern: Pattern,
    /// Base values of the lockable params (UI-set), indexed by
    /// [`LockParam::index`]: `[gain, pan, decay, tone, pitch]`. p-locks override
    /// these per step; `effective = override ?? base` (ADR 0001 §3a).
    base: [f32; N_LOCK_PARAMS],
    /// Last applied effective value per param, so knob re-cooks only fire on a
    /// real change. Seeded to NaN so the first block applies the base.
    applied: [f32; N_LOCK_PARAMS],
    /// Pre-allocated mono render scratch (sized at construction).
    mono: Vec<f32>,
}

impl Track {
    /// A track defaulting to a `Kick/Tone` engine and an empty pattern, using
    /// the given (shared) swap mailbox so the main thread can hand it engines.
    pub fn new(sample_rate: f32, max_block: usize, swap: Arc<EngineSwap>) -> Self {
        Self {
            engine: Box::new(KickTone::with_default_patch(sample_rate)),
            swap,
            pattern: Pattern::default(),
            // gain 1, pan 0, knobs at midpoint (matches the faceplate defaults).
            base: [1.0, 0.0, 0.5, 0.5, 0.5],
            applied: [f32::NAN; N_LOCK_PARAMS],
            mono: vec![0.0; max_block],
        }
    }

    /// Set a lockable param's base value (from a UI command).
    pub fn set_base(&mut self, param: LockParam, value: f32) {
        self.base[param.index()] = value;
    }

    /// Resolve this block's effective params (`override ?? base`) and apply any
    /// that changed: gain/pan feed [`Track::pan_gains`]; knob changes re-cook the
    /// engine. Called once per block before render. Allocation-free.
    pub fn apply_effective(&mut self, lane: &LaneState) {
        for p in 0..N_LOCK_PARAMS {
            let eff = lane.override_value(p).unwrap_or(self.base[p]);
            if eff != self.applied[p] {
                self.applied[p] = eff;
                match p {
                    0 | 1 => {} // gain / pan: read from `applied` in pan_gains
                    2 => self.engine.set_knob(Knob::Decay, eff),
                    3 => self.engine.set_knob(Knob::Tone, eff),
                    4 => self.engine.set_knob(Knob::Pitch, eff),
                    _ => {}
                }
            }
        }
    }

    /// Install a pending off-thread engine swap, if any. Audio-thread,
    /// allocation-free. Returns `true` when a swap happened.
    #[inline]
    pub fn poll_swap(&mut self) -> bool {
        self.swap.try_install(&mut self.engine)
    }

    /// Equal-power pan gains `(left, right)`, from the effective gain/pan.
    #[inline]
    pub fn pan_gains(&self) -> (f32, f32) {
        let gain = self.applied[0];
        let angle = (self.applied[1].clamp(-1.0, 1.0) * 0.5 + 0.5) * std::f32::consts::FRAC_PI_2;
        (gain * angle.cos(), gain * angle.sin())
    }

    /// Render this track for the block into its mono scratch, applying the
    /// pre-scheduled `hits` sample-accurately by slicing the render at each hit's
    /// frame offset. `hits` are frame-ordered and clamped to `[0, frames]` by the
    /// scheduler ([`crate::lane`]). Allocation-free.
    pub fn render_with_hits(&mut self, hits: &[Hit], frames: usize) {
        let frames = frames.min(self.mono.len());
        let engine: &mut dyn TrackEngine = &mut *self.engine;
        let mono = &mut self.mono[..frames];

        let mut pos = 0usize;
        for h in hits {
            let f = h.frame.min(frames);
            if f > pos {
                engine.render(&mut mono[pos..f]);
                pos = f;
            }
            engine.on_trig(h.note, h.velocity);
        }
        engine.render(&mut mono[pos..frames]);
    }

    /// Mix the rendered mono scratch into the stereo bus with gain/pan.
    #[inline]
    pub fn mix_into(&self, out_l: &mut [f32], out_r: &mut [f32], frames: usize) {
        let frames = frames.min(self.mono.len()).min(out_l.len()).min(out_r.len());
        let (gl, gr) = self.pan_gains();
        for f in 0..frames {
            let s = self.mono[f];
            out_l[f] += s * gl;
            out_r[f] += s * gr;
        }
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.engine.set_sample_rate(sample_rate);
    }

    pub fn reset(&mut self) {
        self.engine.reset();
    }
}
